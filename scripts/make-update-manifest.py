#!/usr/bin/env python3
"""Writes the update manifest (latest.json) the app's updater reads.

    make-update-manifest.py --version 0.2.0 --base-url URL --out latest.json DIR

DIR holds the signed update archives of a release, named
`Speachy-<version>-macos-<arch>.app.tar.gz`, each with a `.sig` file beside
it. `--base-url` is where the archives will be downloadable from. Exits
with an error if DIR holds no signed archive.
"""
import argparse
import datetime
import json
import pathlib
import sys

# Our file-name suffix -> the updater's name for the platform.
PLATFORMS = {"macos-arm64": "darwin-aarch64", "macos-x64": "darwin-x86_64"}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--notes", default="", help="text shown to the user with the update")
    parser.add_argument("--out", required=True)
    parser.add_argument("dir")
    args = parser.parse_args()

    platforms = {}
    for suffix, platform in PLATFORMS.items():
        archive = pathlib.Path(args.dir) / f"Speachy-{args.version}-{suffix}.app.tar.gz"
        signature = archive.with_name(archive.name + ".sig")
        if not archive.is_file() or not signature.is_file():
            continue
        platforms[platform] = {
            "signature": signature.read_text().strip(),
            "url": f"{args.base_url.rstrip('/')}/{archive.name}",
        }
    if not platforms:
        sys.exit(f"error: no signed update archive for version {args.version} in {args.dir}")

    manifest = {
        "version": args.version,
        "notes": args.notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    pathlib.Path(args.out).write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {args.out} for {', '.join(sorted(platforms))}")


if __name__ == "__main__":
    main()
