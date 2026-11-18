#!/usr/bin/env python3
"""把一个插件 crate 的 release 构建打成插件商城的 bundle ZIP。

用法：
    python scripts/package.py --plugin whois --platform windows-x86_64
    python scripts/package.py --plugin musiccard --platform android-aarch64 \
        --version 0.1.0 --target-dir target
    python scripts/package.py --plugin rusto --platform windows-x86_64 \
        --assets models --assets NOTICE.md

约定（与 FuyuBot 插件商城一致）：
- 产物名：`<id>-<version>-<platform>.zip`；
- ZIP 根就是插件目录内容，`plugin.json` 在根，不额外套目录；
- `plugin.json` 的 `version` / `platforms` / `entry` 按目标平台改写；
- 动态库放在 `entry` 指向的位置（多数插件在根，musiccard 在 `bin/`）；
- crate 根下的 `models/` 与 `NOTICE.md` **自动带入**（存在才带，CI 不用加参数）；
- `--assets`（可重复）再追加别的额外文件，路径**相对 crate 根**，ZIP 内保持原路径；
  两者都没有时包内仍然只有 `plugin.json` + 动态库（旧插件行为不变）；
- 商城上限：单包 ≤50MB、解压 ≤200MB、文件数 ≤2000，超了直接报错（不留半成品）。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sys
import zipfile
from pathlib import Path
from zipfile import ZipInfo

ROOT = Path(__file__).resolve().parent.parent

# platform -> (动态库扩展名, 交叉编译 target triple)
PLATFORMS = {
    "windows-x86_64": ("dll", None),
    "linux-x86_64": ("so", None),
    "macos-aarch64": ("dylib", None),
    "android-aarch64": ("so", "aarch64-linux-android"),
    "android-x86_64": ("so", "x86_64-linux-android"),
}

SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)

# 插件商城的 bundle 上限：单包 ≤50MB、解压 ≤200MB、≤2000 个文件。
MAX_ARCHIVE_BYTES = 50 * 1024 * 1024
MAX_UNPACKED_BYTES = 200 * 1024 * 1024
MAX_ENTRIES = 2000

# crate 根下存在就自动打进 bundle 的资产：内置模型目录与许可声明。
AUTO_ASSETS = ("models", "NOTICE.md")


def normalize_version(value: str) -> str:
    version = value.removeprefix("v")
    if not SEMVER_RE.fullmatch(version):
        raise SystemExit(f"版本号必须是 semver，例如 1.2.0：{value!r}")
    return version


def crate_dir(plugin: str) -> Path:
    path = ROOT / "crates" / f"fuyubot-plugin-{plugin}"
    if not (path / "Cargo.toml").is_file():
        raise SystemExit(f"找不到插件 crate：{path}")
    return path


def manifest_field(text: str, section: str, key: str) -> str:
    """读取 Cargo.toml 里某个 section 的字符串字段（只支持本项目用到的简单写法）。"""
    in_section = False
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("[") and line.endswith("]"):
            in_section = line == f"[{section}]"
            continue
        if not in_section or line.startswith("#") or "=" not in line:
            continue
        name, _, value = line.partition("=")
        if name.strip() != key:
            continue
        value = value.split("#", 1)[0].strip()
        match = re.fullmatch(r'"([^"]*)"', value)
        if not match:
            raise SystemExit(f"Cargo.toml [{section}] {key} 不是简单字符串：{value!r}")
        return match.group(1)
    raise SystemExit(f"Cargo.toml 缺少 [{section}] {key}")


def load_plugin(plugin: str) -> tuple[Path, dict, str, str]:
    crate = crate_dir(plugin)
    cargo = (crate / "Cargo.toml").read_text(encoding="utf-8")
    package_name = manifest_field(cargo, "package", "name")
    if package_name != f"fuyubot-plugin-{plugin}":
        raise SystemExit(f"crate 名与目录不一致：{package_name}")
    version = manifest_field(cargo, "package", "version")
    library_name = manifest_field(cargo, "lib", "name")

    manifest_path = crate / "plugin.json"
    if not manifest_path.is_file():
        raise SystemExit(f"找不到 {manifest_path}")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema") != 1:
        raise SystemExit("plugin.json 的 schema 必须是 1")
    if manifest.get("id") != plugin:
        raise SystemExit(f"plugin.json 的 id 与 --plugin 不一致：{manifest.get('id')!r}")
    # 当前宿主支持 ABI v5（旧）/ v6（新），bundle 按插件自己声明的版本打。
    if manifest.get("kind") != "native" or manifest.get("abi") not in (5, 6):
        raise SystemExit("plugin.json 必须声明 native kind 和 ABI 5 或 6")
    return crate, manifest, version, library_name


def library_file(library_name: str, extension: str) -> str:
    if extension == "dll":
        return f"{library_name}.dll"
    return f"lib{library_name}.{extension}"


def entry_for(manifest: dict, library: str) -> str:
    """把 plugin.json 里的 Windows entry 换成目标平台的库文件名，保留目录前缀。"""
    windows_entry = manifest.get("entry", "")
    prefix = Path(windows_entry).parent
    if prefix == Path("."):
        return library
    if prefix.is_absolute() or ".." in prefix.parts:
        raise SystemExit(f"plugin.json 的 entry 不合法：{windows_entry!r}")
    return (prefix / library).as_posix()


def add_file(archive: zipfile.ZipFile, path: Path, arcname: str) -> None:
    info = ZipInfo(arcname, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, path.read_bytes())


def asset_entries(crate: Path, assets: list[str]) -> list[tuple[str, Path]]:
    """展开 --assets：返回 (ZIP 内路径, 源文件)，ZIP 内路径相对 crate 根。

    目录会递归展开；解析符号链接后仍须落在 crate 内，避免把仓库外的文件打进
    发布包。同一个文件被重复指定时只打一次。
    """
    root = crate.resolve()
    entries: dict[str, Path] = {}
    for raw in assets:
        text = raw.strip().replace("\\", "/")
        if not text or text == ".":
            raise SystemExit("--assets 不能是空路径")
        relative = Path(text)
        if relative.is_absolute() or ".." in relative.parts:
            raise SystemExit(f"--assets 必须是 crate 内的相对路径：{raw!r}")
        source = root / relative
        if not source.exists():
            raise SystemExit(f"--assets 指向的资产不存在：{source}")
        resolved = source.resolve()
        if resolved != root and root not in resolved.parents:
            raise SystemExit(f"--assets 不能指向 crate 之外：{raw!r}")
        files = (
            sorted(path for path in resolved.rglob("*") if path.is_file())
            if resolved.is_dir()
            else [resolved]
        )
        if not files:
            raise SystemExit(f"--assets 目录里没有文件：{source}")
        for file in files:
            entries.setdefault(file.relative_to(root).as_posix(), file)
    return sorted(entries.items())


def check_limits(count: int, unpacked: int) -> None:
    """预检商城上限：文件数与解压体积（在写 ZIP 之前就失败，不留半成品）。"""
    if count > MAX_ENTRIES:
        raise SystemExit(
            f"bundle 文件数 {count} 超过商城上限 {MAX_ENTRIES}；--assets 收窄一点"
        )
    if unpacked > MAX_UNPACKED_BYTES:
        raise SystemExit(
            f"bundle 解压后 {unpacked / (1024 * 1024):.1f} MiB 超过商城上限 "
            f"{MAX_UNPACKED_BYTES / (1024 * 1024):.0f} MiB；--assets 收窄一点"
        )


def check_archive_size(path: Path) -> None:
    """检查压缩包本身体积；超限就删掉半成品，不留在 dist/ 里被误发。"""
    size = path.stat().st_size
    if size > MAX_ARCHIVE_BYTES:
        path.unlink()
        raise SystemExit(
            f"bundle 压缩后 {size / (1024 * 1024):.1f} MiB 超过商城上限 "
            f"{MAX_ARCHIVE_BYTES / (1024 * 1024):.0f} MiB；--assets 收窄一点"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description="打包一个 FuyuBot 原生插件 bundle")
    parser.add_argument("--plugin", required=True, help="插件 id，等于 plugin.json 的 id")
    parser.add_argument("--platform", required=True, choices=sorted(PLATFORMS))
    parser.add_argument("--version", help="默认取 Cargo.toml 的 package.version；必须一致")
    parser.add_argument("--target-dir", default="target", help="cargo 的 target 根目录")
    parser.add_argument("--output-dir", default="dist")
    parser.add_argument(
        "--assets",
        action="append",
        default=[],
        metavar="PATH",
        help="追加打进 bundle 的资产（可重复），相对 crate 根；crate 根的 models/ 与 NOTICE.md 会自动带入",
    )
    args = parser.parse_args()

    crate, manifest, cargo_version, library_name = load_plugin(args.plugin)
    version = normalize_version(args.version) if args.version else cargo_version
    if version != cargo_version:
        raise SystemExit(
            f"命令行版本 {version} 与 {crate.name}/Cargo.toml 的 {cargo_version} 不一致"
        )
    if manifest.get("version") != cargo_version:
        raise SystemExit(
            f"{crate.name}/plugin.json 的 version 与 Cargo.toml 不一致："
            f"{manifest.get('version')!r} != {cargo_version!r}"
        )
    declared = manifest.get("platforms") or []
    if args.platform not in declared:
        raise SystemExit(
            f"{crate.name}/plugin.json 没有声明 {args.platform}；已声明：{declared}"
        )

    extension, triple = PLATFORMS[args.platform]
    library = library_file(library_name, extension)
    target_root = ROOT / args.target_dir
    release_dir = target_root / triple / "release" if triple else target_root / "release"
    library_path = release_dir / library
    if not library_path.is_file():
        raise SystemExit(f"找不到 {library_path}；请先执行 cargo build --release")

    entry = entry_for(manifest, library)

    # 额外资产：crate 根的 models/ 与 NOTICE.md 自动带入（存在才带），--assets 追加。
    # ZIP 内路径都保持相对 crate 根的原样。
    automatic = [name for name in AUTO_ASSETS if (crate / name).exists()]
    assets = asset_entries(crate, [*automatic, *args.assets])
    for arcname, _ in assets:
        if arcname == "plugin.json" or arcname == entry:
            raise SystemExit(f"--assets 里的 {arcname} 与 plugin.json / 动态库冲突")

    output_dir = ROOT / args.output_dir
    bundle_dir = output_dir / f"bundle-{args.platform}"
    if bundle_dir.exists():
        shutil.rmtree(bundle_dir)
    (bundle_dir / Path(entry).parent).mkdir(parents=True, exist_ok=True)
    shutil.copy2(library_path, bundle_dir / entry)
    for arcname, source in assets:
        destination = bundle_dir / arcname
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)

    manifest["version"] = version
    manifest["entry"] = entry
    manifest["platforms"] = [args.platform]
    plugin_json = bundle_dir / "plugin.json"
    plugin_json.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    # 商城上限先预检一遍：plugin.json + 动态库 + 资产。
    check_limits(
        count=2 + len(assets),
        unpacked=(
            plugin_json.stat().st_size
            + (bundle_dir / entry).stat().st_size
            + sum(source.stat().st_size for _, source in assets)
        ),
    )

    archive_path = output_dir / f"{args.plugin}-{version}-{args.platform}.zip"
    with zipfile.ZipFile(archive_path, "w") as archive:
        add_file(archive, plugin_json, "plugin.json")
        add_file(archive, bundle_dir / entry, entry)
        for arcname, source in assets:
            add_file(archive, source, arcname)

    expected = {"plugin.json", entry} | {arcname for arcname, _ in assets}
    with zipfile.ZipFile(archive_path) as archive:
        names = set(archive.namelist())
        if names != expected:
            raise SystemExit(
                f"ZIP 内容不正确，期望 {sorted(expected)}，实际 {sorted(names)}"
            )
        # 解压体积按 ZIP 里记录的真实大小再核一遍，防止和预检算漏对不上。
        check_limits(
            count=len(names),
            unpacked=sum(info.file_size for info in archive.infolist()),
        )
        packaged = json.loads(archive.read("plugin.json"))
        if packaged.get("version") != version:
            raise SystemExit("ZIP 中 version 与构建版本不一致")
        if packaged.get("platforms") != [args.platform]:
            raise SystemExit("ZIP 中 platforms 与目标平台不一致")
        if packaged.get("entry") != entry:
            raise SystemExit("ZIP 中 entry 与包内动态库路径不一致")

    check_archive_size(archive_path)

    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    size_mb = archive_path.stat().st_size / (1024 * 1024)
    print(f"[OK] {archive_path.relative_to(ROOT)} ({size_mb:.1f} MiB)")
    if automatic:
        print(f"     自动带入 crate 根的 {'、'.join(automatic)}")
    if assets:
        print(f"     含 {len(assets)} 个额外资产文件")
    print(f"     sha256 {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
