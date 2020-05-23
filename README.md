# Btrfs-mirror-subvolumes

Mirror btrfs subvolumes to another file system while preserving sharing.

## Use case

 * Btrfs snapshots protect against unintentional filesystem modifications.
 * RAID 1 protects against hardware failure.
 * Mirroring a filesystem protects against software bugs that render the
   entire filesystem unmountable.

The script in this repository facilitates the third point.

## Requirements

Suppose we have a btrfs filesystem at `/fs1`, with a `/current` subvolume,
and daily read-only snapshots of that subvolume under a `/snapshots` subvolume.
Suppose furthermore, that we have a btrfs filesystem `/fs2` with a `/snapshots`
subvolume.

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

 * We want `/fs2/snapshots` have the same contents as `/fs1/snapshots`.

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

## Alternatives ruled out

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
