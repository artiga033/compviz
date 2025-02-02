use core::fmt;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    env,
    fmt::Display,
    fs::{self, File},
    ops::AddAssign,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::anyhow;
use humansize::{FormatSize, BINARY};
mod btrfs;
mod ffi;

#[derive(Debug, Default)]
struct ExtentInfo {
    pub disk_bytes: usize,
    pub uncompressed_bytes: usize,
    pub referenced_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CompressionType(u8);
impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self.0 {
                0 => "none",
                1 => "zlib",
                2 => "lzo",
                3 => "zstd",
                _ => return write!(f, "unknown({})", self.0),
            }
        )
    }
}
#[derive(Debug, Default)]
struct Statistic {
    pub extent_info: HashMap<CompressionType, ExtentInfo>,
    pub n_files: usize,
    pub n_extents: usize,
    pub n_refs: usize,
    pub n_inline: usize,
}

impl AddAssign<&Statistic> for Statistic {
    fn add_assign(&mut self, rhs: &Statistic) {
        self.n_files += rhs.n_files;
        self.n_extents += rhs.n_extents;
        self.n_refs += rhs.n_refs;
        self.n_inline += rhs.n_inline;
        for (compression, info) in rhs.extent_info.iter() {
            let self_info = self.extent_info.entry(*compression).or_default();
            self_info.disk_bytes += info.disk_bytes;
            self_info.uncompressed_bytes += info.uncompressed_bytes;
            self_info.referenced_bytes += info.referenced_bytes;
        }
    }
}
impl Statistic {
    pub fn table(&self) -> impl Display + '_ {
        struct T<'a>(&'a Statistic);
        impl Display for T<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                writeln!(
                    f,
                    "Processed {} files, {} regular extents ({} refs), {} inline.",
                    self.0.n_files, self.0.n_extents, self.0.n_refs, self.0.n_inline
                )?;
                macro_rules! print_table {
                    ($f:expr, $col1:expr, $col2:expr, $col3:expr, $col4:expr, $col5:expr) => {
                        writeln!(
                            $f,
                            "{:<10} {:<8} {:<12} {:<12} {:<12}",
                            $col1, $col2, $col3, $col4, $col5
                        )?;
                    };
                }
                print_table!(
                    f,
                    "Type",
                    "Perc",
                    "Disk Usage",
                    "Uncompressed",
                    "Referenced"
                );
                let total =
                    self.0
                        .extent_info
                        .values()
                        .fold(ExtentInfo::default(), |mut acc, e| {
                            acc.disk_bytes += e.disk_bytes;
                            acc.uncompressed_bytes += e.uncompressed_bytes;
                            acc.referenced_bytes += e.referenced_bytes;
                            acc
                        });

                let percent = format!(
                    "{:.2}%",
                    (total.disk_bytes as f64 / total.referenced_bytes as f64) * 100.0
                );

                print_table!(
                    f,
                    "TOTAL",
                    percent,
                    total.disk_bytes.format_size(BINARY),
                    total.uncompressed_bytes.format_size(BINARY),
                    total.referenced_bytes.format_size(BINARY)
                );
                for (compression, info) in self.0.extent_info.iter() {
                    let percent = format!(
                        "{:.2}%",
                        (info.disk_bytes as f64 / info.referenced_bytes as f64) * 100.0
                    );
                    print_table!(
                        f,
                        compression.to_string(),
                        percent,
                        info.disk_bytes.format_size(BINARY),
                        info.uncompressed_bytes.format_size(BINARY),
                        info.referenced_bytes.format_size(BINARY)
                    );
                }

                Ok(())
            }
        }
        T(self)
    }
}

struct FileExtentsEnumerator {
    args: btrfs::btrfs_ioctl_search_args_v2_64KB,
    seen_extents: Arc<Mutex<HashSet<u64>>>,
    stat: Statistic,
}
impl FileExtentsEnumerator {
    pub fn with_shared(seen_extents: Arc<Mutex<HashSet<u64>>>) -> Self {
        Self {
            args: btrfs::btrfs_ioctl_search_args_v2_64KB::new_search_file_extent_data(0),
            stat: Statistic::default(),
            seen_extents,
        }
    }
    pub fn work_on_file(
        &mut self,
        path: impl AsRef<Path>,
        file_type: fs::FileType,
    ) -> anyhow::Result<()> {
        let path = path.as_ref();
        if file_type.is_file() {
            self.stat.n_files += 1;
            let f = File::open(path)?;
            self.args.set_search_file_extent_data(f.metadata()?.ino());
            let mut iter = btrfs::get_file_extents_with(f, &mut self.args)?;
            for extent in iter.into_iter() {
                let extent = extent?;
                let info = self
                    .stat
                    .extent_info
                    .entry(CompressionType(extent.compression()))
                    .or_default();
                if extent.type_() == btrfs::BtrfsFileExtentType::Inline {
                    info.disk_bytes += extent.disk_num_bytes() as usize;
                    info.uncompressed_bytes += extent.ram_bytes() as usize;
                    info.referenced_bytes += extent.ram_bytes() as usize;
                    self.stat.n_inline += 1;
                    return Ok(());
                }
                // okay to unwrap as only INLINE extents will have a None, and we return early
                if self
                    .seen_extents
                    .lock()
                    .unwrap()
                    .insert(extent.disk_bytenr().unwrap())
                {
                    info.disk_bytes += extent.disk_num_bytes() as usize;
                    info.uncompressed_bytes += extent.ram_bytes() as usize;
                    self.stat.n_extents += 1;
                }
                info.referenced_bytes += extent.num_bytes() as usize;
                self.stat.n_refs += 1;
            }
        } else if file_type.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let file_type = entry.file_type()?;
                rayon::spawn(move || {
                    T_ENUMRATOR.with_borrow_mut(|e| {
                        if let Err(err) = e.work_on_file(entry.path(), file_type) {
                            eprintln!("Error: {}", err);
                        }
                    })
                });
            }
        }
        Ok(())
    }
}
thread_local! {
    static T_ENUMRATOR: RefCell<FileExtentsEnumerator> = panic!("thread local enumrator not initialized");
}
fn main() -> anyhow::Result<()> {
    let stat = Mutex::new(Statistic::default());
    let shared_hashset = Arc::new(Mutex::new(HashSet::new()));
    rayon::ThreadPoolBuilder::new()
        .num_threads(
            if let Ok(Ok(env_var)) = env::var("RAYON_NUM_THREADS").map(|s| s.parse()) {
                env_var
            } else {
                let cpus = std::thread::available_parallelism()
                    .map(|x| x.get())
                    .unwrap_or(1);
                match cpus {
                    0..=6 => cpus,
                    24..usize::MAX => 24,
                    _ => cpus / 2 + 1,
                }
            },
        )
        .build_scoped(
            |thread| {
                T_ENUMRATOR.set(FileExtentsEnumerator::with_shared(shared_hashset.clone()));
                thread.run();
                T_ENUMRATOR.with_borrow(|e| {
                    *stat.lock().unwrap() += &e.stat;
                });
            },
            |pool| {
                pool.install(|| -> anyhow::Result<()> {
                    let path = std::env::args()
                        .nth(1)
                        .ok_or_else(|| anyhow!("Missing argument"))?;
                    let metadata: fs::Metadata = fs::metadata(&path)?;
                    T_ENUMRATOR.with_borrow_mut(|e| e.work_on_file(path, metadata.file_type()))
                })
            },
        )??;
    println!("{}", stat.lock().unwrap().table());
    Ok(())
}
