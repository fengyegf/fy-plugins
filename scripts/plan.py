#!/usr/bin/env python3
"""计算某个插件的发布矩阵，供 GitHub Actions 使用。

用法：
    python scripts/plan.py --plugin whois [--version 0.1.2]
    python scripts/plan.py --plugin whois --github-output

输出 JSON（stdout）：
    {"id": "...", "version": "...", "platforms": [...],
     "matrix": {"include": [{"platform": "...", "os": "...", "target": null,
                             "android_abi": null}, ...]}}

`--github-output` 会把 `id` / `version` / `matrix` 追加到 `$GITHUB_OUTPUT`。
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from package import PLATFORMS, load_plugin, normalize_version  # noqa: E402

# platform -> (runner, cargo target, android ABI)
RUNNERS = {
    "windows-x86_64": ("windows-latest", "", ""),
    "linux-x86_64": ("ubuntu-latest", "", ""),
    "macos-aarch64": ("macos-15", "", ""),
    "android-aarch64": ("ubuntu-latest", "aarch64-linux-android", "arm64-v8a"),
    "android-x86_64": ("ubuntu-latest", "x86_64-linux-android", "x86_64"),
}


def main() -> int:
    parser = argparse.ArgumentParser(description="计算插件发布矩阵")
    parser.add_argument("--plugin", required=True)
    parser.add_argument("--version", help="校验版本与 Cargo.toml / plugin.json 一致")
    parser.add_argument("--github-output", action="store_true")
    args = parser.parse_args()

    crate, manifest, cargo_version, _ = load_plugin(args.plugin)
    version = normalize_version(args.version) if args.version else cargo_version
    if version != cargo_version:
        raise SystemExit(
            f"tag 版本 {version} 与 {crate.name}/Cargo.toml 的 {cargo_version} 不一致"
        )

    platforms = manifest.get("platforms") or []
    unknown = [p for p in platforms if p not in PLATFORMS]
    if unknown:
        raise SystemExit(f"plugin.json 声明了未知平台：{unknown}")
    if not platforms:
        raise SystemExit("plugin.json 没有声明 platforms")

    include = [
        {
            "platform": platform,
            "os": RUNNERS[platform][0],
            "target": RUNNERS[platform][1],
            "android_abi": RUNNERS[platform][2],
        }
        for platform in platforms
    ]
    result = {
        "id": args.plugin,
        "version": version,
        "platforms": platforms,
        "matrix": {"include": include},
    }
    text = json.dumps(result, ensure_ascii=False)
    print(text)

    if args.github_output:
        output = os.environ.get("GITHUB_OUTPUT")
        if not output:
            raise SystemExit("--github-output 需要 GITHUB_OUTPUT 环境变量")
        with open(output, "a", encoding="utf-8") as handle:
            handle.write(f"id={args.plugin}\n")
            handle.write(f"version={version}\n")
            handle.write(f"matrix={json.dumps(result['matrix'])}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
