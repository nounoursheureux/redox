//To use this, please install zfs-fuse
use redox::*;

use self::dsl_dataset::DslDatasetPhys;
use self::dsl_dir::DslDirPhys;
use self::from_bytes::FromBytes;

pub mod dsl_dataset;
pub mod dsl_dir;
pub mod from_bytes;
pub mod lzjb;
pub mod nvpair;
pub mod nvstream;
pub mod xdr;
pub mod zap;

#[repr(packed)]
pub struct VdevLabel {
    pub blank: [u8; 8 * 1024],
    pub boot_header: [u8; 8 * 1024],
    pub nv_pairs: [u8; 112 * 1024],
    pub uberblocks: [Uberblock; 128],
}

impl FromBytes for VdevLabel { }

#[derive(Copy, Clone, Debug)]
#[repr(packed)]
pub struct Uberblock {
    pub magic: u64,
    pub version: u64,
    pub txg: u64,
    pub guid_sum: u64,
    pub timestamp: u64,
    pub rootbp: BlockPtr,
}

impl Uberblock {
    pub fn magic_little() -> u64 {
        return 0x0cb1ba00;
    }

    pub fn magic_big() -> u64 {
        return 0x00bab10c;
    }

}

impl FromBytes for Uberblock {
    fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() >= mem::size_of::<Uberblock>() {
            let uberblock = unsafe { ptr::read(data.as_ptr() as *const Uberblock) };
            if uberblock.magic == Uberblock::magic_little() {
                return Some(uberblock);
            } else if uberblock.magic == Uberblock::magic_big() {
                return Some(uberblock);
            } else if uberblock.magic > 0 {
                println!("Unknown Magic: {:X}", uberblock.magic as usize);
            }
        }

        None
    }
}

#[derive(Copy, Clone)]
#[repr(packed)]
pub struct DVAddr {
    pub vdev: u64,
    pub offset: u64,
}

impl DVAddr {
    /// Sector address is the offset plus two vdev labels and one boot block (4 MB, or 8192 sectors)
    pub fn sector(&self) -> u64 {
        self.offset() + 0x2000
    }

    pub fn gang(&self) -> bool {
        if self.offset&0x8000000000000000 == 1 {
            true
        } else {
            false
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset & 0x7FFFFFFFFFFFFFFF
    }

    pub fn asize(&self) -> u64 {
        (self.vdev & 0xFFFFFF) + 1
    }
}

impl fmt::Debug for DVAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "DVAddr {{ offset: {:X}, gang: {}, asize: {:X} }}\n",
                    self.offset(), self.gang(), self.asize()));
        Ok(())
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(packed)]
pub struct BlockPtr {
    pub dvas: [DVAddr; 3],
    pub flags_size: u64,
    pub padding: [u64; 3],
    pub birth_txg: u64,
    pub fill_count: u64,
    pub checksum: [u64; 4],
}

impl BlockPtr {
    pub fn level(&self) -> u64 {
        (self.flags_size >> 56) & 0x7F
    }

    pub fn object_type(&self) -> u64 {
        (self.flags_size >> 48) & 0xFF
    }

    pub fn checksum(&self) -> u64 {
        (self.flags_size >> 40) & 0xFF
    }

    pub fn compression(&self) -> u64 {
        (self.flags_size >> 32) & 0xFF
    }

    pub fn lsize(&self) -> u64 {
        (self.flags_size & 0xFFFF) + 1
    }

    pub fn psize(&self) -> u64 {
        ((self.flags_size >> 16) & 0xFFFF) + 1
    }
}

impl FromBytes for BlockPtr { }

#[derive(Copy, Clone, Debug)]
#[repr(packed)]
pub struct Gang {
    pub bps: [BlockPtr; 3],
    pub padding: [u64; 14],
    pub magic: u64,
    pub checksum: u64,
}

impl Gang {
    pub fn magic() -> u64 {
        return 0x117a0cb17ada1002;
    }
}

#[repr(packed)]
pub struct DNodePhys {
    pub object_type: u8,
    pub indblkshift: u8, // ln2(indirect block size)
    pub nlevels: u8, // 1=blkptr->data blocks
    pub nblkptr: u8, // length of blkptr
    pub bonus_type: u8, // type of data in bonus buffer
    pub checksum: u8, // ZIO_CHECKSUM type
    pub compress: u8, // ZIO_COMPRESS type
    pub flags: u8, // DNODE_FLAG_*
    pub data_blk_sz_sec: u16, // data block size in 512b sectors
    pub bonus_len: u16, // length of bonus
    pub pad2: [u8; 4],

    // accounting is protected by dirty_mtx
    pub maxblkid: u64, // largest allocated block ID
    pub used: u64, // bytes (or sectors) of disk space

    pub pad3: [u64; 4],

    blkptr_bonus: [u8; 448],
}

impl DNodePhys {
    pub fn get_blockptr<'a>(&self, i: usize) -> &'a BlockPtr {
        unsafe { mem::transmute(&self.blkptr_bonus[i*128]) }
    }

    pub fn get_bonus(&self) -> &[u8] {
        &self.blkptr_bonus[(self.nblkptr as usize)*128..]
    }
}

impl FromBytes for DNodePhys { }

impl fmt::Debug for DNodePhys {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "DNodePhys {{ object_type: {:X}, nlevels: {:X}, nblkptr: {:X}, bonus_type: {:X}, bonus_len: {:X}}}\n",
                    self.object_type, self.nlevels, self.nblkptr, self.bonus_type, self.bonus_len));
        Ok(())
    }
}

#[repr(packed)]
pub struct ObjectSetPhys {
    meta_dnode: DNodePhys,
    zil_header: ZilHeader,
    os_type: u64,
    //pad: [u8; 360],
}

impl FromBytes for ObjectSetPhys { }

#[repr(packed)]
pub struct ZilHeader {
    claim_txg: u64,
    replay_seq: u64,
    log: BlockPtr,
}

pub struct ZfsHelper {
    disk: File,
}

impl ZfsHelper {
    //TODO: Error handling
    pub fn read(&mut self, start: usize, length: usize) -> Vec<u8> {
        let mut ret: Vec<u8> = vec![0; length*512];

        self.disk.seek(SeekFrom::Start(start * 512));
        self.disk.read(&mut ret);

        return ret;
    }

    pub fn write(&mut self, block: usize, data: &[u8; 512]) {
        self.disk.seek(SeekFrom::Start(block * 512));
        self.disk.write(data);
    }

    pub fn read_dva(&mut self, dva: &DVAddr) -> Vec<u8> {
        self.read(dva.sector() as usize, dva.asize() as usize)
    }

    pub fn read_block(&mut self, block_ptr: &BlockPtr) -> Option<Vec<u8>> {
        let data = self.read_dva(&block_ptr.dvas[0]);
        match block_ptr.compression() {
            2 => {
                // compression off
                Some(data)
            },
            1 | 3 => {
                // lzjb compression
                let mut decompressed = vec![0; (block_ptr.lsize()*512) as usize];
                lzjb::decompress(&data, &mut decompressed);
                Some(decompressed)
            },
            _ => None,
        }
    }

    pub fn read_type<T: FromBytes>(&mut self, block_ptr: &BlockPtr) -> Option<T> {
        self.read_type_array(block_ptr, 0)
    }

    pub fn read_type_array<T: FromBytes>(&mut self, block_ptr: &BlockPtr, offset: usize) -> Option<T> {
        let data = self.read_block(block_ptr);
        data.and_then(|data| T::from_bytes(&data[offset*mem::size_of::<T>()..]))
    }

    pub fn uber(&mut self) -> Option<Uberblock> {
        let mut newest_uberblock: Option<Uberblock> = None;
        for i in 0..128 {
            match Uberblock::from_bytes(&self.read(256 + i * 2, 2)) {
                Some(uberblock) => {
                    let mut newest = false;
                    match newest_uberblock {
                        Some(previous) => {
                            if uberblock.txg > previous.txg {
                                newest = true;
                            }
                        }
                        None => newest = true,
                    }

                    if newest {
                        newest_uberblock = Some(uberblock);
                    }
                }
                None => (), //Invalid uberblock
            }
        }
        return newest_uberblock;
    }
}

pub struct ZFS {
    pub helper: ZfsHelper,
    pub uberblock: Uberblock, // The active uberblock
    fs_objset: ObjectSetPhys,
    master_node: DNodePhys,
    root: u64,
}

impl ZFS {
    pub fn new(disk: File) -> Option<Self> {
        let mut zfs_helper = ZfsHelper { disk: disk };

        let uberblock = zfs_helper.uber().unwrap();

        let mos_dva = uberblock.rootbp.dvas[0];
        let mos: ObjectSetPhys = zfs_helper.read_type(&uberblock.rootbp).unwrap();
        let mos_block_ptr1 = mos.meta_dnode.get_blockptr(0);
        let mos_block_ptr2 = mos.meta_dnode.get_blockptr(1);
        let mos_block_ptr3 = mos.meta_dnode.get_blockptr(2);

        // 2nd dnode in MOS points at the root dataset zap
        let dnode1: DNodePhys = zfs_helper.read_type_array(&mos_block_ptr1, 1).unwrap();

        let root_ds: zap::MZapWrapper = zfs_helper.read_type(dnode1.get_blockptr(0)).unwrap();

        let root_ds_dnode: DNodePhys =
            zfs_helper.read_type_array(&mos_block_ptr1, root_ds.chunks[0].value as usize).unwrap();

        let dsl_dir = DslDirPhys::from_bytes(root_ds_dnode.get_bonus()).unwrap();
        let head_ds_dnode: DNodePhys =
            zfs_helper.read_type_array(&mos_block_ptr1, dsl_dir.head_dataset_obj as usize).unwrap();

        let root_dataset = DslDatasetPhys::from_bytes(head_ds_dnode.get_bonus()).unwrap();

        let fs_objset: ObjectSetPhys = zfs_helper.read_type(&root_dataset.bp).unwrap();

        let mut indirect: BlockPtr = zfs_helper.read_type_array(fs_objset.meta_dnode.get_blockptr(0), 0).unwrap();
        while indirect.level() > 0 {
            indirect = zfs_helper.read_type_array(&indirect, 0).unwrap();
        }

        // Master node is always the second object in the object set
        let mut master_node: DNodePhys = zfs_helper.read_type_array(&indirect, 1).unwrap();
        let master_node_zap: zap::MZapWrapper = zfs_helper.read_type(master_node.get_blockptr(0)).unwrap();
        let mut root = None;
        for chunk in &master_node_zap.chunks {
            if chunk.name().unwrap() == "ROOT" {
                root = Some(chunk.value);
            }
        }

        Some(ZFS {
            helper: zfs_helper,
            uberblock: uberblock,
            fs_objset: fs_objset,
            master_node: master_node,
            root: root.unwrap(),
        })
    }

    pub fn read_file(&mut self, path: &str) -> Option<String> {
        let red = [127, 127, 255, 255];
        // This is the sweet spot: We've found the root directory, so we can now traverse
        // the directory tree.
        let mut indirect: BlockPtr = self.helper.read_type_array(self.fs_objset.meta_dnode.get_blockptr(0), 0).unwrap();
        while indirect.level() > 0 {
            indirect = self.helper.read_type_array(&indirect, 0).unwrap();
        }
        let mut cur_node: DNodePhys = self.helper.read_type_array(&indirect,
                                                                  self.root as usize).unwrap();
 
        let path = path.trim_matches('/');
        for folder in path.split('/') {
            let dir_contents: zap::MZapWrapper = self.helper.read_type(cur_node.get_blockptr(0)).unwrap();
            let mut next_dir = None;
            for chunk in &dir_contents.chunks {
                if chunk.name().unwrap().len() == 0 {
                    break;
                }
                if chunk.name().unwrap() == folder {
                    next_dir = Some(chunk.value);
                    break;
                }
            }
            match next_dir {
                Some(next_dir) => {
                    cur_node = self.helper.read_type_array(&indirect,
                                                           next_dir as usize).unwrap();
                    if cur_node.object_type == 0x13 {
                        break;
                    }
                },
                None => {
                    println_color!(red, "ERROR: path doesn't exist: {}", path);
                    return None;
                },
            }
        }
        let file_contents = self.helper.read_block(cur_node.get_blockptr(0)).unwrap();
        let file_contents: Vec<u8> = file_contents.into_iter().take_while(|c| *c != 0).collect();
        String::from_utf8(file_contents).ok()
    }

    pub fn ls(&mut self, path: &str) -> Option<Vec<String>> {
        let red = [127, 127, 255, 255];
        let mut indirect: BlockPtr = self.helper.read_type_array(self.fs_objset.meta_dnode.get_blockptr(0), 0).unwrap();
        while indirect.level() > 0 {
            indirect = self.helper.read_type_array(&indirect, 0).unwrap();
        }
        let mut cur_node: DNodePhys = self.helper.read_type_array(&indirect,
                                                                  self.root as usize).unwrap();
        let path = path.trim_matches('/');
        let path_end_index = path.rfind('/').map(|i| i+1).unwrap_or(0);
        let path_end = &path[path_end_index..];
        for folder in path.split('/') {
            let dir_contents: zap::MZapWrapper = self.helper.read_type(cur_node.get_blockptr(0)).unwrap();
            let ls: Vec<String> =
                dir_contents.chunks
                            .iter()
                            .map(|x| x.name().unwrap().to_string())
                            .take_while(|x| x.len() > 0)
                            .collect();
            if path == "" {
                return Some(ls);
            }
            let mut next_dir = None;
            for chunk in &dir_contents.chunks {
                if chunk.name().unwrap().len() == 0 {
                    break;
                }
                if chunk.name().unwrap() == folder {
                    next_dir = Some(chunk.value);
                    break;
                }
            }
            match next_dir {
                Some(next_dir) => {
                    cur_node = self.helper.read_type_array(&indirect,
                                                    next_dir as usize).unwrap();
                },
                None => {
                    println_color!(red, "ERROR: path doesn't exist: {}", path);
                    return None;
                },
            }
            if folder == path_end {
                let dir_contents: zap::MZapWrapper = self.helper.read_type(cur_node.get_blockptr(0)).unwrap();
                let ls: Vec<String> =
                    dir_contents.chunks
                                .iter()
                                .map(|x| x.name().unwrap().to_string())
                                .take_while(|x| x.len() > 0)
                                .collect();
                return Some(ls);
            }
        }

        None
    }
}

//TODO: Find a way to remove all the to_string's
pub fn main() {
    console_title("ZFS");

    let red = [255, 127, 127, 255];
    let green = [127, 255, 127, 255];
    let blue = [127, 127, 255, 255];

    println!("Type open zfs.img to open the image file");

    let mut zfs_option: Option<ZFS> = None;

    while let Some(line) = readln!() {
        let mut args: Vec<String> = Vec::new();
        for arg in line.split(' ') {
            args.push(arg.to_string());
        }

        if let Some(command) = args.get(0) {
            println!("# {}", line);

            let mut close = false;
            match zfs_option {
                Some(ref mut zfs) => {
                    if command == "uber" {
                        let ref uberblock = zfs.uberblock;
                        //128 KB of ubers after 128 KB of other stuff
                        println_color!(green, "Newest Uberblock {:X}", zfs.uberblock.magic);
                        println!("Version {}", uberblock.version);
                        println!("TXG {}", uberblock.txg);
                        println!("GUID {:X}", uberblock.guid_sum);
                        println!("Timestamp {}", uberblock.timestamp);
                        println!("ROOTBP[0] {:?}", uberblock.rootbp.dvas[0]);
                        println!("ROOTBP[1] {:?}", uberblock.rootbp.dvas[1]);
                        println!("ROOTBP[2] {:?}", uberblock.rootbp.dvas[2]);
                    } else if command == "vdev_label" {
                        match VdevLabel::from_bytes(&zfs.helper.read(0, 256 * 2)) {
                            Some(ref mut vdev_label) => {
                                let mut xdr = xdr::MemOps::new(&mut vdev_label.nv_pairs);
                                let nv_list = nvstream::decode_nv_list(&mut xdr);
                                println_color!(green, "Got nv_list:\n{:?}", nv_list);
                            },
                            None => { println_color!(red, "Couldn't read vdev_label"); },
                        }
                    } else if command == "file" {
                        match args.get(1) {
                            Some(arg) => {
                                let file = zfs.read_file(arg.as_str());
                                match file {
                                    Some(file) => {
                                        println!("File contents: {}", file);
                                    },
                                    None => println_color!(red, "Failed to read file"),
                                }
                            }
                            None => println_color!(red, "Usage: file <path>"),
                        }
                    } else if command == "ls" {
                        match args.get(1) {
                            Some(arg) => {
                                let ls = zfs.ls(arg.as_str());
                                match ls {
                                    Some(ls) => {
                                        for item in &ls {
                                            print!("{}\t", item);
                                        }
                                    },
                                    None => println_color!(red, "Failed to read directory"),
                                }
                            }
                            None => println_color!(red, "Usage: ls <path>"),
                        }
                    } else if command == "mos" {
                        let ref uberblock = zfs.uberblock;
                        let mos_dva = uberblock.rootbp.dvas[0];
                        println_color!(green, "DVA: {:?}", mos_dva);
                        println_color!(green, "type: {:X}", uberblock.rootbp.object_type());
                        println_color!(green, "checksum: {:X}", uberblock.rootbp.checksum());
                        println_color!(green, "compression: {:X}", uberblock.rootbp.compression());
                        println!("Reading {} sectors starting at {}", mos_dva.asize(), mos_dva.sector());
                        let obj_set: Option<ObjectSetPhys> = zfs.helper.read_type(&uberblock.rootbp);
                        if let Some(ref obj_set) = obj_set {
                            println_color!(green, "Got meta object set");
                            println_color!(green, "os_type: {:X}", obj_set.os_type);
                            println_color!(green, "meta dnode: {:?}\n", obj_set.meta_dnode);

                            println_color!(green, "Reading MOS...");
                            let mos_block_ptr = obj_set.meta_dnode.get_blockptr(0);
                            let mos_array_dva = mos_block_ptr.dvas[0];

                            println_color!(green, "DVA: {:?}", mos_array_dva);
                            println_color!(green, "type: {:X}", mos_block_ptr.object_type());
                            println_color!(green, "checksum: {:X}", mos_block_ptr.checksum());
                            println_color!(green, "compression: {:X}", mos_block_ptr.compression());
                            println!("Reading {} sectors starting at {}", mos_array_dva.asize(), mos_array_dva.sector());
                            let dnode: Option<DNodePhys> = zfs.helper.read_type_array(&mos_block_ptr, 1);
                            println_color!(green, "Got MOS dnode array");
                            println_color!(green, "dnode: {:?}", dnode);

                            if let Some(ref dnode) = dnode {
                                println_color!(green, "Reading object directory zap object...");
                                let zap_obj: Option<zap::MZapWrapper> = zfs.helper.read_type(dnode.get_blockptr(0));
                                println!("{:?}", zap_obj);
                            }
                        }
                    } else if command == "dump" {
                        match args.get(1) {
                            Some(arg) => {
                                let sector = arg.to_num();
                                println_color!(green, "Dump sector: {}", sector);

                                let data = zfs.helper.read(sector, 1);
                                for i in 0..data.len() {
                                    if i % 32 == 0 {
                                        print!("\n{:X}:", i);
                                    }
                                    if let Some(byte) = data.get(i) {
                                        print!(" {:X}", *byte);
                                    } else {
                                        println!(" !");
                                    }
                                }
                                print!("\n");
                            }
                            None => println_color!(red, "No sector specified!"),
                        }
                    } else if command == "close" {
                        println_color!(red, "Closing");
                        close = true;
                    } else {
                        println_color!(blue, "Commands: uber vdev_label mos file ls dump close");
                    }
                }
                None => {
                    if command == "open" {
                        match args.get(1) {
                            Some(arg) => {
                                match File::open(arg) {
                                    Some(file) => {
                                        println_color!(green, "Open: {}", arg);
                                        zfs_option = ZFS::new(file);
                                    },
                                    None => println_color!(red, "File not found!"),
                                }
                            }
                            None => println_color!(red, "No file specified!"),
                        }
                    } else {
                        println_color!(blue, "Commands: open");
                    }
                }
            }
            if close {
                zfs_option = None;
            }
        }
    }
}
