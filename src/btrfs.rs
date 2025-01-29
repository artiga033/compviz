use core::fmt;
use std::{cell::RefCell, io, os::fd::AsRawFd};

use libc::ioctl;

pub use crate::ffi::btrfs_ioctl_search_args_v2_64KB;
use crate::ffi::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BtrfsFileExtentType {
    INLINE = 0,
    REGULAR = 1,
    PREALLOC = 2,
    UNKNOWN = 255,
}
impl From<u8> for BtrfsFileExtentType {
    fn from(v: u8) -> Self {
        match v {
            0 => BtrfsFileExtentType::INLINE,
            1 => BtrfsFileExtentType::REGULAR,
            2 => BtrfsFileExtentType::PREALLOC,
            _ => BtrfsFileExtentType::UNKNOWN,
        }
    }
}
#[derive(Debug)]
pub struct BtrfsFileExtentItem<'a> {
    ptr: *const btrfs_file_extent_item,
    len: usize,
    read: RefCell<Option<btrfs_file_extent_item>>,
    phantom: std::marker::PhantomData<&'a ()>,
}
impl BtrfsFileExtentItem<'_> {
    const INLINE_DATA_OFFSET: usize = std::mem::offset_of!(btrfs_file_extent_item, disk_bytenr);
    fn ensure_read(&self) -> &Self {
        self.read
            .borrow_mut()
            .get_or_insert_with(|| unsafe { self.ptr.read_unaligned() });
        self
    }
    #[inline]
    pub fn generation(&self) -> u64 {
        self.ensure_read().read.borrow().unwrap().generation
    }
    #[inline]
    pub fn ram_bytes(&self) -> u64 {
        self.ensure_read().read.borrow().unwrap().ram_bytes
    }
    #[inline]
    pub fn compression(&self) -> u8 {
        self.ensure_read().read.borrow().unwrap().compression
    }
    #[inline]
    pub fn encryption(&self) -> u8 {
        self.ensure_read().read.borrow().unwrap().encryption
    }
    #[inline]
    pub fn type_(&self) -> BtrfsFileExtentType {
        self.ensure_read().read.borrow().unwrap().type_.into()
    }
    /// Only non-inline extent has this field.  
    /// For inline extent, this is None.
    #[inline]
    pub fn disk_bytenr(&self) -> Option<u64> {
        match self.type_() {
            BtrfsFileExtentType::INLINE => None,
            _ => Some(self.ensure_read().read.borrow().unwrap().disk_bytenr),
        }
    }
    #[inline]
    /// Only non-inline extent has this field.  
    /// For inline extent, this is calculated from `metadata length - previous meaningful fields`
    pub fn disk_num_bytes(&self) -> u64 {
        match self.type_() {
            BtrfsFileExtentType::INLINE => (self.len - Self::INLINE_DATA_OFFSET) as u64,
            _ => self.ensure_read().read.borrow().unwrap().disk_num_bytes,
        }
    }
    /// Only non-inline extent has this field.  
    /// For inline extent, this is None.
    #[inline]
    pub fn offset(&self) -> Option<u64> {
        match self.type_() {
            BtrfsFileExtentType::INLINE => None,
            _ => Some(self.ensure_read().read.borrow().unwrap().offset),
        }
    }
    /// Only non-inline extent has this field.  
    /// For inline extent, this is same as ram_bytes
    #[inline]
    pub fn num_bytes(&self) -> u64 {
        match self.type_() {
            BtrfsFileExtentType::INLINE => self.ram_bytes(),
            _ => self.ensure_read().read.borrow().unwrap().num_bytes,
        }
    }
}

impl fmt::Display for BtrfsFileExtentItem<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.ensure_read().read.borrow(),)
    }
}
pub struct BtrfsFileExtentIterator<'a> {
    fd: std::fs::File,
    args: &'a mut btrfs_ioctl_search_args_v2_64KB,
    buf_offset: isize,
}
impl<'a> Iterator for &mut BtrfsFileExtentIterator<'a> {
    type Item = Result<BtrfsFileExtentItem<'a>, std::io::Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.buf_offset < 0 {
            let ret = unsafe {
                // SAFETY: self.args and self.fd are valid as long as self is alive
                ioctl(
                    self.fd.as_raw_fd(),
                    BTRFS_IOC_TREE_SEARCH_V2_ULONG,
                    &(*self.args),
                )
            };
            if ret < 0 {
                return Some(Err(io::Error::last_os_error()));
            }
            self.buf_offset = 0;
        }
        let bp = unsafe {
            // SAFETY:
            // self.args.buf is valid as long as self is alive
            // ioctl won't destroy the buffer anyway
            self.args.buf.as_mut_ptr().byte_offset(self.buf_offset)
        };
        if self.args.key.nr_items == 0 {
            return None;
        }

        let (head, extent_item) = unsafe {
            let head = bp
                .cast::<btrfs_ioctl_search_header>()
                .as_ref()
                .unwrap_unchecked();
            let bp = bp.byte_add(size_of::<btrfs_ioctl_search_header>());
            let extent_item = bp
                .cast::<btrfs_file_extent_item>()
                .as_ref()
                .unwrap_unchecked();
            // set the offset to next item
            //
            // Actually, there is no need to read and follow the `head.len` field,
            // as for non-inline files, the item is of the fixed size.
            // For inline files, there will be only one extent item, so self will only iterate once, such that the offset is meaningless.
            //
            // Considering that there's very little performance sacrifice, let's just do this.
            self.buf_offset +=
                (size_of::<btrfs_ioctl_search_header>() + head.len as usize) as isize;
            (head, extent_item)
        };
        // nr_items minus one
        self.args.key.nr_items -= 1;
        if self.args.key.nr_items == 0 {
            let unused_size = self.args.buf_size as usize - self.buf_offset as usize;
            // normally an item is of 85 bytes(32 header + 53 file_extent_item), for non-inline file.
            // It may be longer if the extent item has inline data, but in that case the file would have only one extent so it's okay.
            // So if the unused_size is less than that,
            // we assumes that the buffer is overflowed and the data is not complete
            // (even though it could be of the case where the buffer is just used up exactly).
            //
            // You may wonder why `ioctl` call does not return an EOVERFLOW?
            // That is returned only when the buffer is too small to hold even one item(<85 bytes).
            // Or else the ioctl call succeeds and the kernel fills the buffer with as many items as it can,
            // and stops when the buffer is full.
            const BUF_ITEM_SIZE: usize = std::mem::size_of::<btrfs_ioctl_search_header>()
                + std::mem::size_of::<btrfs_file_extent_item>();
            if unused_size < BUF_ITEM_SIZE {
                // set buf offset to -1 so that the next iteration will call ioctl again
                self.buf_offset = -1;
                // set the offset to search for subsequent items
                self.args.key.min_offset = head.offset + 1;
                // reset the number of items to search
                self.args.key.nr_items = u32::MAX;
            }
        }

        Some(Ok(BtrfsFileExtentItem {
            ptr: extent_item,
            len: head.len as usize,
            read: RefCell::new(None),
            phantom: std::marker::PhantomData,
        }))
    }
}

/// It's the users' responsibility to pass the `args` as the struct is quite large.  
/// So it's the user to determine whether to reuse args if a large amount of files are to be searched.
pub fn get_file_extents_with(
    fd: std::fs::File,
    args: &mut btrfs_ioctl_search_args_v2_64KB,
) -> Result<BtrfsFileExtentIterator<'_>, std::io::Error> {
    Ok(BtrfsFileExtentIterator {
        fd,
        args,
        buf_offset: -1,
    })
}

impl btrfs_ioctl_search_args_v2_64KB {
    /// Create [btrfs_ioctl_search_args_v2_64KB] with the fixed buffer and buffer size,
    /// max and min object id set to the given ino, and min/max type set to [BTRFS_EXTENT_DATA_KEY],
    /// leaving all other max/min fieldsset to their extremum.
    ///
    /// This is ideal for searching all extents of a file by its inode number.
    #[inline]
    pub fn new_search_file_extent_data(ino: u64) -> btrfs_ioctl_search_args_v2_64KB {
        btrfs_ioctl_search_args_v2_64KB {
            buf: [0; 65536],
            buf_size: 65536,
            key: btrfs_ioctl_search_key {
                tree_id: 0,
                max_objectid: ino,
                min_objectid: ino,
                min_offset: u64::MIN,
                max_offset: u64::MAX,
                min_transid: u64::MIN,
                max_transid: u64::MAX,
                // Only search for EXTENT_DATA_KEY
                min_type: BTRFS_EXTENT_DATA_KEY,
                max_type: BTRFS_EXTENT_DATA_KEY,
                nr_items: u32::MAX,

                unused: 0,
                unused1: 0,
                unused2: 0,
                unused3: 0,
                unused4: 0,
            },
        }
    }
    /// mutate self.key to as if like a newly created [btrfs_ioctl_search_args_v2_64KB] from [btrfs_ioctl_search_args_v2_64KB::new_search_file_extent_data]
    pub fn set_search_file_extent_data(&mut self, ino: u64) {
        self.buf_size = 65536;
        self.key.tree_id = 0;
        self.key.max_objectid = ino;
        self.key.min_objectid = ino;
        self.key.min_offset = u64::MIN;
        self.key.max_offset = u64::MAX;
        self.key.min_transid = u64::MIN;
        self.key.max_transid = u64::MAX;
        self.key.min_type = BTRFS_EXTENT_DATA_KEY;
        self.key.max_type = BTRFS_EXTENT_DATA_KEY;
        self.key.nr_items = u32::MAX;
    }
}
