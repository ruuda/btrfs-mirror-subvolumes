# Btrfs-mirror-subvolumes

Mirror btrfs subvolumes to another file system while preserving sharing.

 * Btrfs snapshots protect against unintentional filesystem modifications.
 * RAID 1 protects against hardware failure.
 * Mirroring a filesystem protects against software bugs that render the
   entire filesystem unmountable.

The tool in this repository facilitates the third point.

 * [Usage](#usage)
 * [Motivation](#motivation)
 * [Implementation](#implementation)
 * [Alternatives](#alternatives)
 * [License](#license)

## Usage

Build the Rust part with [Cargo][install-rust]:

    cargo build --release

Perform an initial full sync of one snapshot, to use as a base for incremental
mirroring:

    export DATE=2020-01-01
    cd /fs2/snapshots
    btrfs subvolume create $DATE
    rsync -a --preallocate --info=progress2 /fs1/snapshots/$DATE/ /fs2/snapshots/$DATE
    btrfs property set -t subvol /fs2/snapshots/$DATE ro true

Do a dry-run of the script:

    ./btrfs-mirror-subvolumes.py --dry-run /fs1/snapshots /fs2/snapshots

Sync a single snapshot (the one closest to the base snapshot):

    ./btrfs-mirror-subvolumes.py --single /fs1/snapshots /fs2/snapshots

If everything looks fine in the new snapshot, mirror all of them sequentially:

    ./btrfs-mirror-subvolumes.py /fs1/snapshots /fs2/snapshots

## Motivation

Suppose we have a btrfs filesystem at `/fs1`, with a `/current` subvolume,
and daily read-only snapshots of that subvolume under a `/snapshots` subvolume.
Suppose furthermore, that we have a second filesystem `/fs2` with a `/snapshots`
subdirectory or subvolume.

    /
    ├── fs1
    │   ├── current
    │   └── snapshots
    │       ├── 2020-01-01
    │       ├── 2020-01-02
    │       ├── 2020-01-03
    │       └── etc.
    └── fs2
        └── snapshots
            ├── 2020-01-01
            ├── 2020-01-02
            ├── 2020-01-03
            └── etc.

 * We want `/fs2/snapshots` to have the same contents as `/fs1/snapshots`.

 * We want `/fs2` to have roughly the same size as `/fs1`. The snapshots under
   `/fs1` have been created incrementally, so they share most of their extents.
   Each snapshot only consumes space for the data that is uniquely in that
   snapshot, and not in any other snapshot. We need to preserve this sharing in
   `/fs2`, to target a similar file system size.

 * `/fs1` and `/fs2` should be as isolated as possible. Low-level replication
   (the extreme case being at the block level, like RAID) has a higher risk of
   replicating bugs. We need higher-level replication, at the file level.

## Implementation

The second file system is again a btrfs file system, mirrored at the file level
with rsync. Some custom tooling sets up snapshots and reflinked file copies in
such a way that a subsequent rsync run can take advantage of sharing.

To mirror a single subvolume:

 * Pick a base snapshot to start from, and `btrfs subvolume snapshot` it,
   mutable at first.

 * Run a custom program to heuristically detect renames and similar files
   between the snapshots, and reflink the originals into the new snapshot with
   a `FICLONE` ioctl. Without this, rsync might detect the relation, but still
   write the bytes into the target file, destroying sharing. (There is
   [a patch][rsync-reflink] that adds reflink support to rsync, but it has seen
   no activity since 2015.)

 * Run rsync with two important flags:

   * `--inplace` to mutate the target file in place, instead of writing to a
     temporary file and renaming that over the old file when the transfer is
     complete. A new temporary file would not share any extents.

   * `--no-whole-file` to enable rsync’s delta algorithm even when the two file
     systems are both local. Writing deltas, rather than rewriting the entire
     file, ensures that the unchanged extents can be shared.

  * After transfer is complete, make the snapshot read-only.

Apart from that, the script drives mirroring all subvolumes until the two file
systems are in sync. It picks the nearest existing snapshot as a base, with a
preference to reconstruct older snapshots from more recent snapshots, because
this fragments the older snapshots rather than the newer ones. (An initial
transfer can `fallocate` the file and then write it in one go. But a delta
mutation of that file in a subsequently synced snapshot will necessarily create
more extents for the changed parts.) At least one mirrored snapshot must exist
already.


## Alternatives

 * `btrfs send` and `btrfs receive` are ruled out for two reasons:

   1. They are extent-based, so they form a lower level replication mechanism
      than file-based replication, with the associated risk.

   2. The man page says:

      > Additionally, receive does not currently do a very good job of
      > validating that an incremental send stream actually makes sense, and it
      > is thus possible for a specially crafted send stream to create a
      > subvolume with reflinks to arbitrary files in the same filesystem.
      > Because of this, users are advised to not use btrfs receive on send
      > streams from untrusted sources, and to protect trusted streams when
      > sending them across untrusted networks.

      This warning in combination with my past experience with btrfs corruption,
      makes me distrustful of `btrfs send` and `btrfs receive` for valuable
      data.

 * Mirroring to a non-btrfs file system, or even a non-Linux operating system,
   would obviously be the best way to guard against btrfs bugs, but maintaining
   a second storage pool with a similar feature set, but implemented differently
   (e.g. ZFS, or XFS on top of LVM and dm-raid) would be a considerable effort
   with its own risks.

## License

Btrfs-mirror-subvolumes is free software licensed under the
[Apache 2.0][apache2] license. Please do not open an issue if
you disagree with the choice of license.

[rsync-reflink]: https://bugzilla.samba.org/show_bug.cgi?id=10170
[install-rust]:  https://forge.rust-lang.org/infra/other-installation-methods.html
[apache2]:       https://www.apache.org/licenses/LICENSE-2.0
[except]:        https://www.gnu.org/licenses/gpl-faq.html#GPLIncompatibleLibs
