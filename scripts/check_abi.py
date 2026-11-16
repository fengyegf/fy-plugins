#!/usr/bin/env python3
"""静态检查一个原生插件动态库导出了哪几个 ABI 符号（ABI v6）。

不 dlopen、不执行插件代码，只读 PE 导出表——所以 ABI v5 时代的
`fuyubot-sdk check` 不适用时，这个脚本仍然能回答「七个符号齐不齐」。

用法：
    python scripts/check_abi.py target/debug/epic.dll [更多 dll ...]

必需：plugin_init / plugin_free_string。
可选（v4 起）：plugin_start / plugin_stop / plugin_free_handle /
              fuyubot_abi_version / plugin_manifest。

退出码：0 = 每个库都过了必需符号；1 = 有缺失；2 = 文件不是可解析的 PE / 不存在。
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

REQUIRED = ("plugin_init", "plugin_free_string")
OPTIONAL = (
    "plugin_start",
    "plugin_stop",
    "plugin_free_handle",
    "fuyubot_abi_version",
    "plugin_manifest",
)
ALL = REQUIRED + OPTIONAL

MACHINES = {
    0x8664: "windows-x86_64",
    0xAA64: "windows-aarch64",
    0x014C: "windows-x86",
}


class PeError(Exception):
    pass


def _u16(data: bytes, off: int) -> int:
    return struct.unpack_from("<H", data, off)[0]


def _u32(data: bytes, off: int) -> int:
    return struct.unpack_from("<I", data, off)[0]


def _cstr(data: bytes, off: int) -> str:
    end = data.find(b"\0", off)
    if end < 0:
        end = len(data)
    return data[off:end].decode("utf-8", "replace")


def pe_exports(path: Path) -> tuple[str, list[str]]:
    """返回 (machine, 导出符号名列表)。"""
    data = path.read_bytes()
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise PeError("不是 PE 文件（缺 MZ 头）")
    pe = _u32(data, 0x3C)
    if data[pe : pe + 4] != b"PE\0\0":
        raise PeError("不是 PE 文件（缺 PE 签名）")

    machine = _u16(data, pe + 4)
    sections = _u16(data, pe + 6)
    opt_size = _u16(data, pe + 20)
    opt = pe + 24
    magic = _u16(data, opt)
    if magic == 0x20B:
        dd = opt + 112
    elif magic == 0x10B:
        dd = opt + 96
    else:
        raise PeError(f"未知的 optional header magic: {magic:#x}")

    export_rva = _u32(data, dd)
    export_size = _u32(data, dd + 4)
    if not export_rva:
        raise PeError("没有导出表")

    headers = opt + opt_size
    table = []
    for i in range(sections):
        base = headers + i * 40
        vsize = _u32(data, base + 8)
        vaddr = _u32(data, base + 12)
        raw_size = _u32(data, base + 16)
        raw_ptr = _u32(data, base + 20)
        table.append((vaddr, max(vsize, raw_size), raw_ptr))

    def rva_to_off(rva: int) -> int:
        for vaddr, size, raw_ptr in table:
            if vaddr <= rva < vaddr + size:
                return raw_ptr + (rva - vaddr)
        raise PeError(f"RVA {rva:#x} 不在任何节里")

    exp = rva_to_off(export_rva)
    count = _u32(data, exp + 24)
    names_rva = _u32(data, exp + 32)
    names = []
    for i in range(count):
        name_rva = _u32(data, rva_to_off(names_rva) + i * 4)
        names.append(_cstr(data, rva_to_off(name_rva)))
    return MACHINES.get(machine, f"machine-{machine:#x}"), names


def check(path: Path) -> bool:
    print(f"== {path}")
    try:
        machine, names = pe_exports(path)
    except (OSError, PeError) as error:
        print(f"  无法解析：{error}")
        return False

    exported = set(names)
    missing = [sym for sym in REQUIRED if sym not in exported]
    present = [sym for sym in ALL if sym in exported]
    absent = [sym for sym in OPTIONAL if sym not in exported]

    print(f"  platform: {machine}")
    print(f"  必需符号: {', '.join(present[:len(REQUIRED)]) or '（无）'}")
    if absent:
        print(f"  缺可选符号: {', '.join(absent)}")
    for extra in sorted(exported - set(ALL)):
        if extra.startswith(("plugin_", "fuyubot_")):
            print(f"  额外导出: {extra}")
    if missing:
        print(f"  ✗ 缺必需符号: {', '.join(missing)}")
        return False
    print(f"  ✓ ABI 符号齐全（{len(present)}/{len(ALL)}）")
    return True


def main(argv: list[str]) -> int:
    # 控制台默认编码（Windows 上是 GBK）不一定能打中文 / 符号，统一按 UTF-8 输出。
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
    if not argv:
        print(__doc__)
        return 2
    files = [Path(arg) for arg in argv]
    ok = True
    for path in files:
        if not path.is_file():
            print(f"== {path}\n  不存在")
            ok = False
            continue
        ok = check(path) and ok
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
