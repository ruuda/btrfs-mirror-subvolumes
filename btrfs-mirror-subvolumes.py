#!/usr/bin/env python3

# btrfs-mirror-subvolumes -- Mirror subvolumes between two btrfs filesystems
# Copyright 2020 Ruud van Asseldonk

# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# A copy of the License has been included in the root of the repository.

"""
Sync read-only subvolumes between two btrfs filesystems in a manner that
preserves as much sharing as possible, without relying on btrfs internals,
to keep the two file systems as isolated as possible, to prevent cascading
corruption in the event of btrfs bugs.

Usage:

    btrfs-mirror-subvolumes.py [--dry-run] [--single] <source-dir> <dest-dir>

Options:

    --dry-run    Print commands that would be executed but do not execute them.
    --single     Stop after syncing one subvolume, even if more are missing.

Source-dir should contain subvolumes named YYYY-MM-DD. Those will be replicated
as read-only subvolumes in the destination directory.

Sync is done with rsync, using a set of flags to optimize for btrfs, in order to
minimize fragmentation and maximize sharing between subvolumes.

It is assumed that subvolumes whose dates are closer together share more, so
when picking a base subvolume for transfer, the existing subvolume with the
nearest date is used as a starting point. This works both forward and backward
in time.

This command might need to run as superuser.
"""

import datetime
import os
import os.path
import subprocess
import sys

from datetime import date
from typing import Optional, List, Set, Tuple


def run(args: List[str], *, dry_run: bool) -> None:
    if dry_run:
        print('Would run', ' '.join(args))
    else:
        subprocess.run(args, check=True)


def list_subvolumes(path: str) -> Set[date]:
    return {
        date.fromisoformat(dirname)
        for dirname
        in os.listdir(path)
    }


def date_distance(x: date, y: date) -> int:
    """
    Return the number of days between x and y if x > y, or twice that if x < y.
    This gives a bias to building backwards, which makes sense because
    subvolumes tend to accumulate growth. Missing files are more likely to be
    present in a future snapshot than in a past snapshot.
    """
    x_day_number = x.toordinal()
    y_day_number = y.toordinal()
    diff = y_day_number - x_day_number
    if diff < 0:
        return diff * -2
    else:
        return diff


def hausdorff_distance(x: date, ys: Set[date]) -> Tuple[int, date]:
    """
    Return the date in ys that is closest to x,
    and the number of days they are apart.
    """
    assert len(ys) > 0, 'Need to have at least one base snapshot to start from.'
    candidates = ((date_distance(x, y), y) for y in ys)
    return min(candidates)


def sync_one(src: str, dst: str, *, dry_run: bool) -> Optional[date]:
    """
    From the snapshots that are present in src and missing in dst, pick the one
    that is closest to an existing snapshot in dst, and sync it. Returns the
    snapshot synced, or none if src and dst are already in sync.
    """
    src_subvols = list_subvolumes(src)
    dst_subvols = list_subvolumes(dst)
    missing_subvols = src_subvols - dst_subvols

    if len(missing_subvols) == 0:
        return None

    # We will sync the *latest* missing subvolume first. The rationale behind
    # this is that data is mostly append-only, and that we prefer fragmenting
    # early snapshots over later snapshots. There is no advantage in rebuilding
    # a file that changed over time in the same order, it will only be
    # fragmented in the later snapshots. Rather, we can sync the final (or at
    # least latest) version, and rebuild the past versions backwards.
    sync_date = max(missing_subvols)
    num_days, base_date = hausdorff_distance(sync_date, dst_subvols)
    base_dir = base_date.isoformat()
    sync_dir = sync_date.isoformat()
    print(f'Syncing {sync_dir}, using {base_dir} as base.')

    # Create a writeable snapshot of the base subvolume.
    cmd = [
        'btrfs', 'subvolume', 'snapshot',
        os.path.join(dst, base_dir),
        os.path.join(dst, sync_dir),
    ]
    run(cmd, dry_run=dry_run)

    print('Waiting for sync of snapshot.')
    # Previously I used "btrfs subvolume sync" instead of "filesystem sync",
    # but that sync process reliably got stuck in an endless ioctl loop where
    # it would call clock_nanosleep to sleep for a second and then a
    # BTRFS_IOC_TREE_SEARCH ioctl, over and over again. A filesystem sync is
    # less buggy.
    cmd_sync = [
        'btrfs', 'filesystem', 'sync',
        os.path.join(dst, sync_dir),
    ]
    run(cmd_sync, dry_run=dry_run)

    cmd = [
        'target/release/reflink-diff',
        'dry-run' if dry_run else 'apply',
        os.path.join(src, base_dir),
        os.path.join(src, sync_dir),
        os.path.join(dst, base_dir),
        os.path.join(dst, sync_dir),
    ]
    subprocess.run(cmd, check=True)

    # Sync into it.
    # Would be nice to use reflink support once that gets mainstream.
    # https://bugzilla.samba.org/show_bug.cgi?id=10170
    cmd = [
        'rsync',
        '-a',
        '--delete-delay',
        '--inplace',
        '--preallocate',
        '--no-whole-file',
        '--fuzzy',
        '--info=copy,del,name1,progress2,stats2',
        os.path.join(src, sync_dir) + '/',
        os.path.join(dst, sync_dir),
    ]
    run(cmd, dry_run=dry_run)

    # Once that is done, make the snapshot readonly.
    cmd = [
        'btrfs', 'property', 'set',
        '-t', 'subvol',
        os.path.join(dst, sync_dir),
        'ro', 'true',
    ]
    run(cmd, dry_run=dry_run)
    run(cmd_sync, dry_run=dry_run)

    return sync_date


def main(src: str, dst: str, *, dry_run: bool, single: bool) -> None:
    while True:
        synced_day = sync_one(src, dst, dry_run=dry_run)

        if synced_day is None:
            break

        if single:
            print('Stopping after one transfer because of --single.')
            break

        if dry_run:
            print('Stopping now to avoid endless loop because of --dry-run.')
            break


if __name__ == '__main__':
    args = list(sys.argv)

    dry_run = '--dry-run' in args
    if dry_run:
        args.remove('--dry-run')

    single = '--single' in args
    if single:
        args.remove('--single')

    if len(args) != 3:
        print(__doc__)
        sys.exit(1)

    else:
        main(args[1], args[2], dry_run=dry_run, single=single)
