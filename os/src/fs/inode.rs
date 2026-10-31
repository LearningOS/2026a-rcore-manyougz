//! `Arc<Inode>` -> `OSInodeInner`: In order to open files concurrently
//! we need to wrap `Inode` into `Arc`,but `Mutex` in `Inode` prevents
//! file systems from being accessed simultaneously
//!
//! `UPSafeCell<OSInodeInner>` -> `OSInode`: for static `ROOT_INODE`,we
//! need to wrap `OSInodeInner` into `UPSafeCell`
use super::{File, Stat, StatMode};
use crate::drivers::BLOCK_DEVICE;
use crate::mm::UserBuffer;
use crate::sync::UPSafeCell;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::*;
use easy_fs::{EasyFileSystem, Inode};
use lazy_static::*;

/// inode in memory
/// A wrapper around a filesystem inode
/// to implement File trait atop
pub struct OSInode {
    readable: bool,
    writable: bool,
    inode_id: u32,
    inner: UPSafeCell<OSInodeInner>,
}
/// The OS inode inner in 'UPSafeCell'
pub struct OSInodeInner {
    offset: usize,
    inode: Arc<Inode>,
}

// 只缓存一份连续写入，换文件、换位置或达到上限时再写回
struct FileWriteBuffer {
    inode_id: u32,
    inode: Arc<Inode>,
    offset: usize,
    data: Vec<u8>,
}

const WRITE_BUFFER_SIZE: usize = 32 * 1024;

fn flush_write_buffer(buffer: &mut Option<FileWriteBuffer>) {
    if let Some(buffer) = buffer.take() {
        assert_eq!(
            buffer.inode.write_at(buffer.offset, &buffer.data),
            buffer.data.len()
        );
    }
}

impl OSInode {
    /// create a new inode in memory
    pub fn new(readable: bool, writable: bool, inode: Arc<Inode>, inode_id: u32) -> Self {
        *OPEN_INODES.exclusive_access().entry(inode_id).or_insert(0) += 1;
        Self {
            readable,
            writable,
            inode_id,
            inner: unsafe { UPSafeCell::new(OSInodeInner { offset: 0, inode }) },
        }
    }
    /// read all data from the inode
    pub fn read_all(&self) -> Vec<u8> {
        flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
        let mut inner = self.inner.exclusive_access();
        let mut buffer: Vec<u8> = Vec::with_capacity(512);
        buffer.resize(512, 0);
        let mut v: Vec<u8> = Vec::new();
        loop {
            let len = inner.inode.read_at(inner.offset, &mut buffer);
            if len == 0 {
                break;
            }
            inner.offset += len;
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }
}

lazy_static! {
    pub static ref ROOT_INODE: Arc<Inode> = {
        let efs = EasyFileSystem::open(BLOCK_DEVICE.clone());
        Arc::new(EasyFileSystem::root_inode(&efs))
    };
    // 只记录打开次数，fork 共享同一个 OSInode，最后一次释放时再减
    static ref OPEN_INODES: UPSafeCell<BTreeMap<u32, usize>> =
        unsafe { UPSafeCell::new(BTreeMap::new()) };
    static ref WRITE_BUFFER: UPSafeCell<Option<FileWriteBuffer>> =
        unsafe { UPSafeCell::new(None) };
}

// 原版 easy-fs 的目录项：前 28 字节是文件名，后 4 字节是 inode 编号
const DIRENT_SZ: usize = 32;
const NAME_SZ: usize = 28;

fn read_dir() -> Vec<[u8; DIRENT_SZ]> {
    let mut entries = Vec::new();
    let mut entry = [0; DIRENT_SZ];
    while ROOT_INODE.read_at(entries.len() * DIRENT_SZ, &mut entry) == DIRENT_SZ {
        entries.push(entry);
    }
    entries
}

fn dirent_name(entry: &[u8; DIRENT_SZ]) -> &str {
    let len = entry[..NAME_SZ]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(NAME_SZ);
    core::str::from_utf8(&entry[..len]).unwrap()
}

fn dirent_inode_id(entry: &[u8; DIRENT_SZ]) -> u32 {
    u32::from_le_bytes(entry[NAME_SZ..].try_into().unwrap())
}

fn find_inode_id(name: &str) -> Option<u32> {
    read_dir()
        .iter()
        .find(|entry| dirent_name(entry) == name)
        .map(dirent_inode_id)
}

fn link_count(inode_id: u32) -> u32 {
    read_dir()
        .iter()
        .filter(|entry| dirent_inode_id(entry) == inode_id)
        .count() as u32
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < NAME_SZ
        && !name.as_bytes().contains(&b'/')
        && !name.as_bytes().contains(&0)
}

fn recycle_inode(inode_id: u32, inode: &Inode) {
    // 通过公开的位图接口回收编号
    let fs = EasyFileSystem::open(BLOCK_DEVICE.clone());
    {
        let fs = fs.lock();
        fs.inode_bitmap.dealloc(&fs.block_device, inode_id as usize);
    }
    // clear 释放数据块和间接索引块，同时把上面的位图修改同步到磁盘
    inode.clear();
}

impl Drop for OSInode {
    fn drop(&mut self) {
        // 先写回，再判断是否回收，不能让缓冲区继续指向已经释放的 inode
        flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
        let mut opened = OPEN_INODES.exclusive_access();
        let count = opened.get_mut(&self.inode_id).unwrap();
        *count -= 1;
        if *count != 0 {
            return;
        }
        opened.remove(&self.inode_id);
        drop(opened);
        // 最后一次关闭，而且已经没有名字指向它，再真回收
        if link_count(self.inode_id) == 0 {
            let inner = self.inner.exclusive_access();
            recycle_inode(self.inode_id, &inner.inode);
        }
    }
}

/// List all apps in the root directory
pub fn list_apps() {
    println!("/**** APPS ****");
    for app in ROOT_INODE.ls() {
        println!("{}", app);
    }
    println!("**************/");
}

bitflags! {
    ///  The flags argument to the open() system call is constructed by ORing together zero or more of the following values:
    pub struct OpenFlags: u32 {
        /// readyonly
        const RDONLY = 0;
        /// writeonly
        const WRONLY = 1 << 0;
        /// read and write
        const RDWR = 1 << 1;
        /// create new file
        const CREATE = 1 << 9;
        /// truncate file size to 0
        const TRUNC = 1 << 10;
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
}

/// Open a file
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
    if !valid_name(name) {
        return None;
    }
    // 其他 fd 重新打开或截断文件前，要先看到已经写入的内容
    flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
    let (readable, writable) = flags.read_write();
    let inode = match ROOT_INODE.find(name) {
        Some(inode) => inode,
        None if flags.contains(OpenFlags::CREATE) => ROOT_INODE.create(name)?,
        None => return None,
    };
    if flags.intersects(OpenFlags::CREATE | OpenFlags::TRUNC) {
        inode.clear();
    }
    let inode_id = find_inode_id(name)?;
    Some(Arc::new(OSInode::new(readable, writable, inode, inode_id)))
}

/// 建立文件硬链接
pub fn link_file(old_name: &str, new_name: &str) -> bool {
    flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
    if old_name == new_name || !valid_name(new_name) || find_inode_id(new_name).is_some() {
        return false;
    }
    let inode_id = match find_inode_id(old_name) {
        Some(inode_id) => inode_id,
        None => return false,
    };
    // 只追加一个目录项，inode 编号沿用旧文件的
    let mut entry = [0; DIRENT_SZ];
    entry[..new_name.len()].copy_from_slice(new_name.as_bytes());
    entry[NAME_SZ..].copy_from_slice(&inode_id.to_le_bytes());
    ROOT_INODE.write_at(read_dir().len() * DIRENT_SZ, &entry);
    true
}

/// 删除文件名对应的目录项
pub fn unlink_file(name: &str) -> bool {
    flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
    let mut entries = read_dir();
    let pos = match entries.iter().position(|entry| dirent_name(entry) == name) {
        Some(pos) => pos,
        None => return false,
    };
    let inode_id = dirent_inode_id(&entries[pos]);
    let inode = ROOT_INODE.find(name).unwrap();
    entries.remove(pos);

    let mut data = Vec::new();
    for entry in entries {
        data.extend_from_slice(&entry);
    }
    ROOT_INODE.clear();
    if !data.is_empty() {
        ROOT_INODE.write_at(0, &data);
    }
    if link_count(inode_id) == 0 && !OPEN_INODES.exclusive_access().contains_key(&inode_id) {
        recycle_inode(inode_id, &inode);
    }
    true
}

impl File for OSInode {
    fn stat(&self) -> Option<Stat> {
        flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
        Some(Stat {
            dev: 0,
            ino: self.inode_id as u64,
            mode: StatMode::FILE,
            nlink: link_count(self.inode_id),
            pad: [0; 7],
        })
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn read(&self, mut buf: UserBuffer) -> usize {
        // 所有 fd 共用这份缓冲，经过硬链接打开的 fd 也能读到最新内容
        flush_write_buffer(&mut WRITE_BUFFER.exclusive_access());
        let mut inner = self.inner.exclusive_access();
        let mut total_read_size = 0usize;
        for slice in buf.buffers.iter_mut() {
            let read_size = inner.inode.read_at(inner.offset, *slice);
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read_size += read_size;
        }
        total_read_size
    }
    fn write(&self, buf: UserBuffer) -> usize {
        let size = buf.len();
        if size == 0 {
            return 0;
        }
        let mut inner = self.inner.exclusive_access();
        let mut buffer = WRITE_BUFFER.exclusive_access();
        if buffer.as_ref().map_or(false, |buffer| {
            buffer.inode_id != self.inode_id || buffer.offset + buffer.data.len() != inner.offset
        }) {
            flush_write_buffer(&mut buffer);
        }
        let pending = buffer.get_or_insert_with(|| FileWriteBuffer {
            inode_id: self.inode_id,
            inode: inner.inode.clone(),
            offset: inner.offset,
            data: Vec::new(),
        });
        // 合并用户缓冲的各个页片段，连续 write 也一起攒到 32KiB，为解决 ci 测试性能超时
        for slice in buf.buffers.iter() {
            pending.data.extend_from_slice(slice);
        }
        inner.offset += size;
        if pending.data.len() >= WRITE_BUFFER_SIZE {
            flush_write_buffer(&mut buffer);
        }
        size
    }
}
