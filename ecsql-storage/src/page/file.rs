use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};

use ecsql_core::page::{PAGE_SIZE, PageId};
use ecsql_os::Vfd;
use thiserror::Error;
static ALIGNED_ZEROED_PAGE: PageBuffer = PageBuffer::zeroed();

#[repr(C, align(4096))]
pub struct PageBuffer([u8; PAGE_SIZE]);
impl PageBuffer {
    pub fn get(&self) -> &[u8; PAGE_SIZE] {
        &self.0
    }
    pub fn get_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        &mut self.0
    }
    pub const fn zeroed() -> Self {
        PageBuffer([0; PAGE_SIZE])
    }
}

#[derive(Debug, Error)]
pub enum PageFileError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("page {} is past the end of the file ({page_count} pages)", page.get())]
    PastEnd { page: PageId, page_count: u32 },
    #[error("page file is full")]
    Full,
}

pub struct PageFile {
    file: Vfd,
    page_count: AtomicU32,
    extend_lock: Mutex<()>,
}

impl PageFile {
    pub fn create(path: &Path) -> Result<Self, PageFileError> {
        let file = Vfd::create_new(path)?;
        Ok(Self {
            file,
            page_count: AtomicU32::new(0),
            extend_lock: Mutex::new(()),
        })
    }

    pub fn open(path: &Path) -> Result<Self, PageFileError> {
        let file = Vfd::open(path)?;
        let page_count =
            u32::try_from(file.size()? / (PAGE_SIZE as u64)).map_err(|_| PageFileError::Full)?;
        Ok(Self {
            file,
            page_count: AtomicU32::new(page_count),
            extend_lock: Mutex::new(()),
        })
    }

    pub fn page_count(&self) -> u32 {
        // we must observe what extend did
        self.page_count.load(Ordering::Acquire)
    }

    pub fn read_page(&self, page: PageId, buf: &mut PageBuffer) -> Result<(), PageFileError> {
        let page_count = self.page_count.load(Ordering::Acquire);
        if page.get() >= page_count {
            return Err(PageFileError::PastEnd { page, page_count });
        }
        self.file.read_exact_at(buf.get_mut(), page.file_offset())?;
        Ok(())
    }

    pub fn write_page(&self, page: PageId, buf: &PageBuffer) -> Result<(), PageFileError> {
        let page_count = self.page_count.load(Ordering::Acquire);
        if page.get() >= page_count {
            return Err(PageFileError::PastEnd { page, page_count });
        }
        self.file.write_all_at(buf.get(), page.file_offset())?;
        Ok(())
    }

    pub fn extend(&self) -> Result<PageId, PageFileError> {
        let _guard = self
            .extend_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let page_id = PageId::new(self.page_count.load(Ordering::Relaxed));
        let new_page_id = page_id.get().checked_add(1).ok_or(PageFileError::Full)?;
        self.file
            .write_all_at(ALIGNED_ZEROED_PAGE.get(), page_id.file_offset())?;
        self.page_count.store(new_page_id, Ordering::Release);

        Ok(page_id)
    }

    pub fn sync(&self) -> Result<(), PageFileError> {
        self.file.sync_data()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::thread;

    use tempfile::{TempDir, tempdir};

    use super::*;

    const TABLE: &str = "table.pages";

    fn filled(byte: u8) -> PageBuffer {
        PageBuffer([byte; PAGE_SIZE])
    }

    fn new_file() -> Result<(TempDir, PageFile), PageFileError> {
        let dir = tempdir()?;
        let file = PageFile::create(&dir.path().join(TABLE))?;
        Ok((dir, file))
    }

    fn file_len(dir: &TempDir) -> u64 {
        std::fs::metadata(dir.path().join(TABLE))
            .expect("page file exists")
            .len()
    }

    /* AI GENERATED TEST */
    #[test]
    fn create_starts_empty() -> Result<(), PageFileError> {
        let (dir, file) = new_file()?;
        assert_eq!(file.page_count(), 0);
        assert_eq!(file_len(&dir), 0);
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn create_fails_if_file_exists() -> Result<(), PageFileError> {
        let (dir, _file) = new_file()?;
        let again = PageFile::create(&dir.path().join(TABLE));
        assert!(matches!(again, Err(PageFileError::Io(_))));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn open_missing_file_fails() -> Result<(), PageFileError> {
        let dir = tempdir()?;
        let opened = PageFile::open(&dir.path().join(TABLE));
        assert!(matches!(opened, Err(PageFileError::Io(_))));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn extend_appends_consecutive_zeroed_pages() -> Result<(), PageFileError> {
        let (dir, file) = new_file()?;
        assert_eq!(file.extend()?, PageId::new(0));
        assert_eq!(file.extend()?, PageId::new(1));
        assert_eq!(file.extend()?, PageId::new(2));
        assert_eq!(file.page_count(), 3);
        assert_eq!(file_len(&dir), 3 * PAGE_SIZE as u64);

        let mut buf = filled(0xAA);
        file.read_page(PageId::new(1), &mut buf)?;
        assert!(buf.0.iter().all(|&byte| byte == 0));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn write_then_read_each_page() -> Result<(), PageFileError> {
        let (_dir, file) = new_file()?;
        for _ in 0..4 {
            file.extend()?;
        }
        for page in 0..4u8 {
            let mut buf = filled(page + 1);
            buf.0[0] = 0xF0 | page;
            buf.0[PAGE_SIZE - 1] = 0xE0 | page;
            file.write_page(PageId::new(u32::from(page)), &buf)?;
        }
        for page in 0..4u8 {
            let mut buf = filled(0);
            file.read_page(PageId::new(u32::from(page)), &mut buf)?;
            assert_eq!(buf.0[0], 0xF0 | page);
            assert_eq!(buf.0[PAGE_SIZE - 1], 0xE0 | page);
            assert!(buf.0[1..PAGE_SIZE - 1].iter().all(|&byte| byte == page + 1));
        }
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn read_past_end_is_an_error() -> Result<(), PageFileError> {
        let (_dir, file) = new_file()?;
        file.extend()?;
        let mut buf = filled(0);
        let result = file.read_page(PageId::new(1), &mut buf);
        assert!(matches!(
            result,
            Err(PageFileError::PastEnd { page, page_count: 1 }) if page == PageId::new(1)
        ));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn write_past_end_is_an_error_and_does_not_grow() -> Result<(), PageFileError> {
        let (dir, file) = new_file()?;
        let result = file.write_page(PageId::new(3), &filled(7));
        assert!(matches!(
            result,
            Err(PageFileError::PastEnd { page, page_count: 0 }) if page == PageId::new(3)
        ));
        assert_eq!(file.page_count(), 0);
        assert_eq!(file_len(&dir), 0);
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn reopen_keeps_pages() -> Result<(), PageFileError> {
        let (dir, file) = new_file()?;
        file.extend()?;
        file.extend()?;
        file.write_page(PageId::new(1), &filled(9))?;
        file.sync()?;
        drop(file);

        let file = PageFile::open(&dir.path().join(TABLE))?;
        assert_eq!(file.page_count(), 2);
        let mut buf = filled(0);
        file.read_page(PageId::new(1), &mut buf)?;
        assert!(buf.0.iter().all(|&byte| byte == 9));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn open_ignores_partial_page_and_extend_overwrites_it() -> Result<(), PageFileError> {
        let dir = tempdir()?;
        let path = dir.path().join(TABLE);
        let mut raw = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        raw.write_all(&[1; PAGE_SIZE])?;
        raw.write_all(&[2; 100])?;
        drop(raw);

        let file = PageFile::open(&path)?;
        assert_eq!(file.page_count(), 1);
        assert_eq!(file.extend()?, PageId::new(1));
        assert_eq!(file_len(&dir), 2 * PAGE_SIZE as u64);

        let mut buf = filled(0xAA);
        file.read_page(PageId::new(1), &mut buf)?;
        assert!(buf.0.iter().all(|&byte| byte == 0));
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn concurrent_extends_get_unique_pages() -> Result<(), PageFileError> {
        const THREADS: u32 = 8;
        const PER_THREAD: u32 = 50;
        let (dir, file) = new_file()?;

        let mut pages: Vec<u32> = thread::scope(|scope| {
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        (0..PER_THREAD)
                            .map(|_| file.extend().map(|page| page.get()))
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("extend thread panicked"))
                .collect::<Result<Vec<_>, _>>()
        })?
        .into_iter()
        .flatten()
        .collect();

        pages.sort_unstable();
        assert_eq!(pages, (0..THREADS * PER_THREAD).collect::<Vec<_>>());
        assert_eq!(file.page_count(), THREADS * PER_THREAD);
        assert_eq!(
            file_len(&dir),
            u64::from(THREADS * PER_THREAD) * PAGE_SIZE as u64
        );
        Ok(())
    }

    /* AI GENERATED TEST */
    #[test]
    fn concurrent_writes_to_different_pages() -> Result<(), PageFileError> {
        const THREADS: u8 = 8;
        let (_dir, file) = new_file()?;
        for _ in 0..THREADS {
            file.extend()?;
        }

        thread::scope(|scope| {
            for page in 0..THREADS {
                let file = &file;
                scope.spawn(move || {
                    for round in 0..20u8 {
                        let value = page.wrapping_mul(31).wrapping_add(round);
                        let target = PageId::new(u32::from(page));
                        file.write_page(target, &filled(value)).expect("write");
                        let mut buf = filled(0);
                        file.read_page(target, &mut buf).expect("read");
                        assert!(buf.0.iter().all(|&byte| byte == value));
                    }
                });
            }
        });
        Ok(())
    }
}
