#!/usr/bin/env python3
"""从 HuggingFace 下载 RustO! 的 PP-OCRv6 Tiny 模型（det / rec / dict）。

为什么单独一个脚本：插件默认在首次识别时自己联网下载模型，但国内网络、离线部署
和 CI 缓存都需要「先把模型拉下来，再和 dll 一起打成离线包」。这里只用标准库
urllib，不引入第三方依赖；下载完打印 sha256，供 package.py --assets 与人工核对。

用法：
    python scripts/fetch-models.py
    python scripts/fetch-models.py --mirror https://hf-mirror.com
    python scripts/fetch-models.py --force --dest crates/fuyubot-plugin-rusto/models

默认预设 ppocrv6-tiny-int8（det.mnn 543,168 B / rec.mnn 2,912,240 B / dict.txt
27,156 B，合计约 3.5 MB）；--preset 可切 ppocrv6-tiny 等其它档位。

落盘结构（与插件 <model_dir>/<preset>/ 的约定一致）：
    crates/fuyubot-plugin-rusto/models/ppocrv6-tiny-int8/det.mnn
    crates/fuyubot-plugin-rusto/models/ppocrv6-tiny-int8/rec.mnn
    crates/fuyubot-plugin-rusto/models/ppocrv6-tiny-int8/dict.txt

格式优先级：`.mnn`（rusto-nn 的原生 MNN kernel，最快）→ `.rten`（通用解释器，慢但
兼容）。上游目录里有 .mnn 就下 .mnn；只有 .rten 时下 .rten，并在输出里明确说明。
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 模型仓库与默认落点（落点相对 crate 根）。
REPO = "byrizki/rusto-models"
DEFAULT_MIRROR = "https://huggingface.co"
DEFAULT_PRESET = "ppocrv6-tiny-int8"
DEFAULT_DEST = ROOT / "crates" / "fuyubot-plugin-rusto" / "models"

# 每类资产按优先级尝试的文件名：先 .mnn，再 .rten。
ASSETS = (
    ("det", ("det.mnn", "det.rten")),
    ("rec", ("rec.mnn", "rec.rten")),
    ("dict", ("dict.txt",)),
)

# 已核实的 sha256（取自 HuggingFace 的 LFS oid）；对不上就删掉，避免半截文件进发布包。
KNOWN_SHA256 = {
    # 默认预设 ppocrv6-tiny-int8
    "ppocrv6-tiny-int8/det.mnn": "232a824bdfcfc3ee6015b1f12ce4263c59e3e40949eb73ac8372e10bfeeb4fdf",
    "ppocrv6-tiny-int8/rec.mnn": "0913d75636e91553346aac062af6eca8430ed5a13dfc7097745d37241cdd9f2d",
    # --preset ppocrv6-tiny 时用这一套
    "ppocrv6-tiny/det.mnn": "7d5badf4799718dbe17113b29d75ffcf37e90fc575fcba4e7b7dd1cb641508a1",
    "ppocrv6-tiny/rec.mnn": "539ad7adcdc2bc92d824903eba323b19205d1091ef7c2f1c119daf7fcfb5d285",
}

USER_AGENT = "fuyubot-plugins-fetch-models/1.0"


class MissingAsset(Exception):
    """上游没有这个文件（404），应该换下一个候选名。"""


def asset_url(mirror: str, preset: str, filename: str) -> str:
    """拼 HuggingFace resolve 直链；--mirror 只是换前缀，路径不变。"""
    return f"{mirror.rstrip('/')}/{REPO}/resolve/main/{preset}/{filename}"


def sha256_of(path: Path) -> str:
    """算文件 sha256（大文件分块读）。"""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url: str, destination: Path, timeout: float) -> int:
    """下载到 .part 再改名（同盘原子），返回字节数；404 抛 MissingAsset。"""
    part = destination.with_name(destination.name + ".part")
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            written = 0
            with part.open("wb") as handle:
                while True:
                    block = response.read(1024 * 1024)
                    if not block:
                        break
                    handle.write(block)
                    written += len(block)
    except urllib.error.HTTPError as error:
        part.unlink(missing_ok=True)
        if error.code == 404:
            raise MissingAsset(url) from error
        raise SystemExit(
            f"下载失败（HTTP {error.code}）：{url}\n"
            f"  镜像或网络问题可换 --mirror，或稍后重试"
        ) from error
    except (urllib.error.URLError, OSError) as error:
        part.unlink(missing_ok=True)
        raise SystemExit(
            f"下载失败：{url}\n  {error}\n"
            f"  国内网络可加 --mirror https://hf-mirror.com"
        ) from error
    part.replace(destination)
    return written


def main() -> int:
    parser = argparse.ArgumentParser(
        description="下载 RustO!/PP-OCRv6 模型到插件 crate（默认 ppocrv6-tiny 的 det/rec/dict）"
    )
    parser.add_argument(
        "--mirror",
        default=DEFAULT_MIRROR,
        help=f"模型源前缀，默认 {DEFAULT_MIRROR}；国内可用 https://hf-mirror.com",
    )
    parser.add_argument(
        "--preset",
        default=DEFAULT_PRESET,
        help=f"模型预设目录名，默认 {DEFAULT_PRESET}（目录名就是 [plugin.fengyegf.rusto] 的 model）",
    )
    parser.add_argument(
        "--dest",
        default=str(DEFAULT_DEST),
        help="模型根目录，其下按 <preset>/ 落盘；相对路径按仓库根解析",
    )
    parser.add_argument("--force", action="store_true", help="已有文件也重新下载")
    parser.add_argument("--timeout", type=float, default=60.0, help="单次 HTTP 超时秒数，默认 60")
    args = parser.parse_args()

    preset = args.preset.strip().strip("/")
    if not preset or "/" in preset or ".." in preset:
        raise SystemExit(f"--preset 不合法：{args.preset!r}")
    if args.timeout <= 0:
        raise SystemExit("--timeout 必须大于 0")
    dest_root = Path(args.dest)
    if not dest_root.is_absolute():
        dest_root = ROOT / dest_root
    target = dest_root / preset
    target.mkdir(parents=True, exist_ok=True)

    print(f"[..] 源   {args.mirror.rstrip('/')}/{REPO}/resolve/main/{preset}/")
    print(f"[..] 落点 {target}")
    print()

    results: list[tuple[str, Path, int, str]] = []
    for role, candidates in ASSETS:
        for filename in candidates:
            relative = f"{preset}/{filename}"
            path = target / filename
            if path.is_file() and not args.force:
                digest = sha256_of(path)
                size = path.stat().st_size
                print(f"[OK] 已存在 {relative}（{size / 1048576:.1f} MiB），跳过；--force 可重下")
                results.append((relative, path, size, digest))
                break
            url = asset_url(args.mirror, preset, filename)
            print(f"[..] 下载 {relative}")
            try:
                size = download(url, path, args.timeout)
            except MissingAsset:
                print(f"[!!] 上游没有 {relative}，换下一个候选")
                continue
            digest = sha256_of(path)
            expected = KNOWN_SHA256.get(relative)
            if expected and expected != digest:
                path.unlink(missing_ok=True)
                raise SystemExit(
                    f"sha256 校验失败：{relative}\n"
                    f"  期望 {expected}\n  实际 {digest}\n"
                    f"  文件已删除，请重试或换 --mirror"
                )
            results.append((relative, path, size, digest))
            print(f"[OK] {relative}  {size / 1048576:.1f} MiB  sha256 {digest}")
            break
        else:
            tried = "、".join(candidates)
            raise SystemExit(
                f"上游找不到 {preset} 的 {role}（试过 {tried}）。\n"
                f"  目录：{args.mirror.rstrip('/')}/{REPO}/tree/main/{preset}\n"
                f"  可换 --mirror，或手工把文件放到 {target}"
            )

    # 记录 sha256：打包/部署后可以拿它核对包里的模型没被换掉。
    sums = target / "SHA256SUMS"
    sums.write_text(
        "\n".join(sorted(f"{digest}  {path.name}" for _, path, _, digest in results)) + "\n",
        encoding="utf-8",
    )

    total = sum(size for _, _, size, _ in results)
    print()
    print(f"[OK] {len(results)} 个文件已就位：{target}")
    for _, path, size, digest in results:
        print(f"     {path.name:<10} {size:>10,} B  {digest}")
    print(f"     {'合计':<8} {total:>10,} B  ({total / 1048576:.1f} MiB)")
    print(f"     sha256 清单：{sums.name}")

    formats = {path.suffix.lstrip(".").lower() for _, path, _, _ in results}
    if "rten" in formats:
        print()
        print("[注意] 这套模型只有 .rten：rusto-nn 的 MNN kernel 用不上，识别会慢一些。")
        print("       上游发布 .mnn 后重新执行并加 --force 即可切回快路径。")

    print()
    print("下一步（打离线包）：")
    print(
        "  python scripts/package.py --plugin rusto --platform windows-x86_64 "
        "--assets models --assets NOTICE.md"
    )
    if preset == DEFAULT_PRESET:
        print(f"  [plugin.fengyegf.rusto] model 默认就是 \"{preset}\"，只需把 model_dir 指到 {dest_root}")
    else:
        print(f"  [plugin.fengyegf.rusto] model = \"{preset}\"（目录名就是预设名）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
