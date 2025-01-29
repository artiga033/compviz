#include <stdint.h>
#include <btrfs/ioctl.h>
#include <btrfs/ctree.h>

struct btrfs_ioctl_search_args_v2_64KB
{
    struct btrfs_ioctl_search_key key;
    uint64_t buf_size;
    uint8_t  buf[65536]; // hardcoded kernel's limit is 16MB
};

unsigned long BTRFS_IOC_TREE_SEARCH_V2_ULONG = BTRFS_IOC_TREE_SEARCH_V2;