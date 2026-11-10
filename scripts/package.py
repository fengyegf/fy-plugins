#!/usr/bin/env python3
"""把一个插件 crate 的 release 构建打成插件商城的 bundle ZIP。

用法：
    python scripts/package.py --plugin whois --platform windows-x86_64
    python scripts/package.py --plugin musiccard --platform android-aarch64 \
        --version 0.1.0 --target-dir target

约定（与 FuyuBot 插件商城一致）：
- 产物名：`<id>-<version>-<platform>.zip`；
- ZIP 根就是插件目录内容，`plugin.json` 在根，不额外套目录；
- `plugin.json` 的 `version` / `platforms` / `entry` 按目标平台改写；
- 动态库放在 `entry` 指向的位置（多数插件在根，musiccard 在 `bin/`）。
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
    if manifest.get("kind") != "native" or manifest.get("abi") != 5:
        raise SystemExit("plugin.json 必须声明 native kind 和 ABI 5")
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


def main() -> int:
    parser = argparse.ArgumentParser(description="打包一个 FuyuBot 原生插件 bundle")
    parser.add_argument("--plugin", required=True, help="插件 id，等于 plugin.json 的 id")
    parser.add_argument("--platform", required=True, choices=sorted(PLATFORMS))
    parser.add_argument("--version", help="默认取 Cargo.toml 的 package.version；必须一致")
    parser.add_argument("--target-dir", default="target", help="cargo 的 target 根目录")
    parser.add_argument("--output-dir", default="dist")
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

    extension, triple = PLATFORMS[args.platform]
    library = library_file(library_name, extension)
    target_root = ROOT / args.target_dir
    release_dir = target_root / triple / "release" if triple else target_root / "release"
    library_path = release_dir / library
    if not library_path.is_file():
        raise SystemExit(f"找不到 {library_path}；请先执行 cargo build --release")

    entry = entry_for(manifest, library)
    output_dir = ROOT / args.output_dir
    bundle_dir = output_dir / f"bundle-{args.platform}"
    if bundle_dir.exists():
        shutil.rmtree(bundle_dir)
    (bundle_dir / Path(entry).parent).mkdir(parents=True, exist_ok=True)
    shutil.copy2(library_path, bundle_dir / entry)

    manifest["version"] = version
    manifest["entry"] = entry
    manifest["platforms"] = [args.platform]
    plugin_json = bundle_dir / "plugin.json"
    plugin_json.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    archive_path = output_dir / f"{args.plugin}-{version}-{args.platform}.zip"
    with zipfile.ZipFile(archive_path, "w") as archive:
        add_file(archive, plugin_json, "plugin.json")
        add_file(archive, bundle_dir / entry, entry)

    with zipfile.ZipFile(archive_path) as archive:
        names = set(archive.namelist())
        if names != {"plugin.json", entry}:
            raise SystemExit(
                f"ZIP 内容不正确，期望 {sorted({'plugin.json', entry})}，实际 {sorted(names)}"
            )
        packaged = json.loads(archive.read("plugin.json"))
        if packaged.get("version") != version:
            raise SystemExit("ZIP 中 version 与构建版本不一致")
        if packaged.get("platforms") != [args.platform]:
            raise SystemExit("ZIP 中 platforms 与目标平台不一致")
        if packaged.get("entry") != entry:
            raise SystemExit("ZIP 中 entry 与包内动态库路径不一致")

    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    size_mb = archive_path.stat().st_size / (1024 * 1024)
    print(f"[OK] {archive_path.relative_to(ROOT)} ({size_mb:.1f} MiB)")
    print(f"     sha256 {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
