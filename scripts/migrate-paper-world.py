#!/usr/bin/env python3
"""Make a separate, vanilla-layout copy of an offline Paper/Spigot server."""

import argparse
import os
from pathlib import Path
import shutil
import stat
import sys

MARKER = ".mcpanel-migration-in-progress"
ARCHIVE = ".mcpanel-paper-originals"
DATA_FILES = ("game_rules.dat", "scheduled_events.dat", "wandering_trader.dat",
              "weather.dat", "world_gen_settings.dat")


def copy_file(source, destination):
    # Exclusive creation prevents an unexpected destination from being replaced.
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    fd = os.open(source, flags)
    with os.fdopen(fd, "rb") as src:
        if not stat.S_ISREG(os.fstat(src.fileno()).st_mode):
            raise ValueError(f"not a regular file: {source}")
        with open(destination, "xb") as dst:
            shutil.copyfileobj(src, dst, length=1024 * 1024)
    shutil.copystat(source, destination, follow_symlinks=False)
    return destination


def reject_links(root):
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            path = Path(directory) / name
            if path.is_symlink():
                raise ValueError(f"symbolic links are not supported: {path.relative_to(root)}")


def migrate(source, destination, world):
    if not world or world in (".", "..", "backups", "logs", ".horizon") or "/" in world or "\\" in world:
        raise ValueError("--world must be a single folder name")
    source = Path(source).resolve(strict=True)
    if not source.is_dir() or not (source / world / "level.dat").is_file():
        raise ValueError("source must contain the selected world and its level.dat")
    destination = Path(destination).absolute()
    parent = destination.parent.resolve(strict=True)
    destination = parent / destination.name
    if source == destination or source in destination.parents or destination in source.parents:
        raise ValueError("source and destination must be separate directories")
    if destination.is_symlink():
        raise ValueError("destination must not be a symbolic link")
    if (source / MARKER).exists() or (source / MARKER).is_symlink():
        raise ValueError("source contains an unfinished migration marker")
    if (source / ARCHIVE).exists() or (source / ARCHIVE).is_symlink():
        raise ValueError(f"source already contains {ARCHIVE}")
    try:
        destination.mkdir(mode=0o700)
    except FileExistsError:
        if not destination.is_dir() or any(destination.iterdir()):
            raise ValueError("destination must be a new or empty directory")
    with (destination / MARKER).open("x", encoding="utf-8") as marker:
        marker.write("Migration is incomplete; do not start this server.\n")

    # Preserve links as links during the copy; never follow one to copy data.
    # Reject the copied links before moving any paths or completing the copy.
    # Copy entries separately so copytree cannot change the panel's existing
    # destination directory permissions to those of the source root.
    for entry in source.iterdir():
        target = destination / entry.name
        if entry.is_symlink():
            raise ValueError(f"symbolic links are not supported: {entry.name}")
        if entry.is_dir():
            shutil.copytree(entry, target, symlinks=True, copy_function=copy_file)
        else:
            copy_file(entry, target)
    reject_links(destination)
    for suffix, dimension in (("_nether", "DIM-1"), ("_the_end", "DIM1")):
        paper = destination / (world + suffix)
        origin = paper / dimension
        if origin.exists():
            if not origin.is_dir():
                raise ValueError(f"dimension is not a directory: {origin}")
            target = destination / world / dimension
            if target.exists():
                raise ValueError(f"conflicting destination: {target}")
            shutil.move(str(origin), str(target))
            archive = destination / ARCHIVE
            archive.mkdir(exist_ok=True)
            shutil.move(str(paper), str(archive / paper.name))

    paper_data = destination / world / "dimensions/minecraft/overworld/data/minecraft"
    vanilla_data = destination / world / "data/minecraft"
    for name in DATA_FILES:
        origin = paper_data / name
        if origin.exists():
            if not origin.is_file():
                raise ValueError(f"world data is not a file: {origin}")
            target = vanilla_data / name
            if target.exists():
                raise ValueError(f"conflicting destination: {target}")
            vanilla_data.mkdir(parents=True, exist_ok=True)
            shutil.move(str(origin), str(target))
    (destination / MARKER).unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, help="stopped Paper/Spigot server directory")
    parser.add_argument("--destination", required=True, help="new or empty Horizon server directory")
    parser.add_argument("--world", required=True, help="the exact level-name from server.properties")
    args = parser.parse_args()
    try:
        migrate(args.source, args.destination, args.world)
    except (OSError, ValueError, shutil.Error) as error:
        print(f"Migration failed: {error}", file=sys.stderr)
        print("The original was not changed. If a partial destination was created, inspect it before retrying.", file=sys.stderr)
        return 1
    print(f"Converted copy created at {args.destination}; the original server was not changed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
