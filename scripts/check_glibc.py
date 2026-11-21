#!/usr/bin/env python3
"""静态读出一个 ELF 动态库要求的 glibc 符号版本（不 dlopen、不执行插件代码）。

为什么需要它：插件是宿主 `dlopen` 进来的动态库，而 `GLIBC_x.y` 符号版本是在**构建机**上
链接时定下来的——用太新的发行版构建，老机器加载会直接失败，宿主的日志只有一句
`dlopen failed`：

    /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.43' not found (required by librusto.so)

所以在打包前用这个脚本把「库要求的最低 glibc」静态读出来，超过发布基线就拦下。

用法：
    python scripts/check_glibc.py target/release/librusto.so
    python scripts/check_glibc.py dist/*.so --max 2.28

退出码：0 = 不高于基线（或没有 glibc 要求）；1 = 要求的 glibc 比基线新；
        2 = 文件不是可解析的 64 位小端 ELF / 不存在。
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

PT_LOAD = 1
PT_DYNAMIC = 2
DT_NULL = 0
DT_STRTAB = 5
DT_VERNEED = 0x6FFFFFFE
DT_VERNEEDNUM = 0x6FFFFFFF


class ElfError(Exception):
    """文件不是能读的 64 位小端 ELF。"""


def _u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def _u64(data: bytes, offset: int) -> int:
    return struct.unpack_from("<Q", data, offset)[0]


def _i64(data: bytes, offset: int) -> int:
    return struct.unpack_from("<q", data, offset)[0]


def _cstr(data: bytes, offset: int) -> str:
    end = data.find(b"\0", offset)
    if end < 0:
        end = len(data)
    return data[offset:end].decode("utf-8", "replace")


def _segments(data: bytes) -> tuple[list[tuple[int, int, int]], tuple[int, int]]:
    """返回 (PT_LOAD 段表 [(vaddr, offset, filesz)], (动态段 offset, size))。"""
    if len(data) < 64 or data[:4] != b"\x7fELF":
        raise ElfError("不是 ELF 文件")
    if data[4] != 2 or data[5] != 1:
        raise ElfError("只支持 64 位小端 ELF（x86_64 / aarch64 的 Linux 目标）")

    phoff = _u64(data, 32)
    phentsize = _u16(data, 54)
    phnum = _u16(data, 56)
    loads: list[tuple[int, int, int]] = []
    dynamic: tuple[int, int] | None = None
    for index in range(phnum):
        base = phoff + index * phentsize
        if base + 56 > len(data):
            raise ElfError("程序头越界")
        kind = _u32(data, base)
        offset = _u64(data, base + 8)
        vaddr = _u64(data, base + 16)
        filesz = _u64(data, base + 32)
        if kind == PT_LOAD:
            loads.append((vaddr, offset, filesz))
        elif kind == PT_DYNAMIC:
            dynamic = (offset, filesz)
    if dynamic is None:
        raise ElfError("没有 PT_DYNAMIC（不是动态库）")
    return loads, dynamic


def _vaddr_to_offset(loads: list[tuple[int, int, int]], vaddr: int) -> int:
    for start, offset, size in loads:
        if start <= vaddr < start + size:
            return offset + (vaddr - start)
    raise ElfError(f"虚拟地址 {vaddr:#x} 不在任何 PT_LOAD 段里")


def _dynamic_entries(data: bytes, dynamic: tuple[int, int]) -> dict[int, list[int]]:
    offset, size = dynamic
    entries: dict[int, list[int]] = {}
    for index in range(size // 16):
        tag = _i64(data, offset + index * 16)
        if tag == DT_NULL:
            break
        entries.setdefault(tag, []).append(_u64(data, offset + index * 16 + 8))
    return entries


def required_glibc(data: bytes) -> list[str]:
    """返回动态库在加载时要求的 glibc 符号版本，例如 ["GLIBC_2.34", "GLIBC_2.28"]。"""
    loads, dynamic = _segments(data)
    entries = _dynamic_entries(data, dynamic)
    if DT_VERNEED not in entries or DT_STRTAB not in entries:
        return []

    strtab = _vaddr_to_offset(loads, entries[DT_STRTAB][0])
    verneed = _vaddr_to_offset(loads, entries[DT_VERNEED][0])
    count = entries.get(DT_VERNEEDNUM, [0])[0]

    names: set[str] = set()
    offset = verneed
    for _ in range(count):
        if offset + 16 > len(data):
            raise ElfError("版本需求表越界")
        _version, aux_count, _file, aux_offset, next_offset = struct.unpack_from(
            "<HHIII", data, offset
        )
        aux = offset + aux_offset
        for _aux in range(aux_count):
            if aux + 16 > len(data):
                raise ElfError("版本需求辅助表越界")
            _hash, _flags, _other, name_offset, aux_next = struct.unpack_from(
                "<IHHII", data, aux
            )
            name = _cstr(data, strtab + name_offset)
            # GLIBC_PRIVATE 是库内部约定，不构成对外版本门槛。
            if name.startswith("GLIBC_") and name != "GLIBC_PRIVATE":
                names.add(name)
            if aux_next == 0:
                break
            aux += aux_next
        if next_offset == 0:
            break
        offset += next_offset
    return sorted(names, key=glibc_version, reverse=True)


def glibc_version(text: str) -> tuple[int, ...]:
    """把 "GLIBC_2.28" / "2.28" 解析成可比较的 (2, 28)。

    老版本号可能有三段（x86_64 的 GLIBC_2.2.5、GLIBC_2.3.4），按整段比较即可：
    (2, 2, 5) < (2, 28) 这种跨段比较在 tuple 语义下正是想要的。
    """
    value = text.removeprefix("GLIBC_").removeprefix("glibc_")
    parts = value.split(".")
    if len(parts) < 2 or not all(part.isdigit() for part in parts):
        raise ValueError(f"不是 glibc 版本号：{text!r}")
    return tuple(int(part) for part in parts)


def main(argv: list[str]) -> int:
    # Windows 的控制台默认不是 UTF-8，直接 print 中文会 UnicodeEncodeError。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="backslashreplace")

    parser = argparse.ArgumentParser(description="检查 ELF 动态库要求的 glibc 版本")
    parser.add_argument("libraries", nargs="+", help="要检查的动态库（.so）")
    parser.add_argument(
        "--max",
        dest="baseline",
        help="允许的最高 glibc 版本（例如 2.28）；省略时只报告不判定",
    )
    args = parser.parse_args(argv)

    try:
        baseline = glibc_version(args.baseline) if args.baseline else None
    except ValueError as error:
        raise SystemExit(str(error)) from error

    failed = False
    for raw in args.libraries:
        path = Path(raw)
        if not path.is_file():
            print(f"== {raw}\n   文件不存在", file=sys.stderr)
            return 2
        try:
            versions = required_glibc(path.read_bytes())
        except (OSError, ElfError, struct.error) as error:
            print(f"== {raw}\n   无法解析：{error}", file=sys.stderr)
            return 2

        print(f"== {path}")
        if not versions:
            print("   没有 glibc 符号版本要求（静态链接或非 glibc 目标）")
            continue

        newest = max(versions, key=glibc_version)
        print(f"   glibc 需求：{', '.join(versions)}")
        print(f"   最高：{newest}")
        if baseline and glibc_version(newest) > baseline:
            print(
                f"   [FAIL] 高于基线 {args.baseline}：这个库在更老的发行版上 dlopen 会失败"
                f"（version `{newest}' not found）"
            )
            failed = True
        else:
            print("   [OK]")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
