use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;

use crate::component::align_up;
use thiserror::Error;

pub const PAGE_SIZE: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(u32);
impl PageId {
    pub fn new(id: u32) -> Self {
        Self(id)
    }
    pub fn file_offset(self) -> u64 {
        u64::from(self.0) * (PAGE_SIZE as u64)
    }
    #[inline]
    pub fn get(&self) -> u32 {
        self.0
    }
}
pub const HEADER_SIZE: usize = 24;

pub const ENTITY_SIZE: usize = size_of::<u64>();

pub const MAX_ROW_SIZE: usize = 2048;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PageLayoutError {
    #[error("row of {size} bytes exceeds the {max}-byte limit")]
    RowTooLarge { size: usize, max: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLayout {
    rows_per_page: usize,
    component_offsets: Box<[usize]>,
}

impl PageLayout {
    pub fn compute(components: &[Layout]) -> Result<Self, PageLayoutError> {
        let mut component_offsets = vec![0usize; components.len()];
        let row_size: usize = components.iter().map(Layout::size).sum();
        if row_size > MAX_ROW_SIZE {
            return Err(PageLayoutError::RowTooLarge {
                size: row_size,
                max: MAX_ROW_SIZE,
            });
        }
        let rows_per_page = (PAGE_SIZE - HEADER_SIZE) / (row_size + ENTITY_SIZE);
        let mut order: Vec<usize> = (0..components.len()).collect();
        order.sort_by_key(|&i| core::cmp::Reverse(components[i].align()));
        let mut cursor = HEADER_SIZE + rows_per_page * ENTITY_SIZE;
        for i in order {
            cursor = align_up(cursor, components[i].align());
            component_offsets[i] = cursor;
            cursor += rows_per_page * components[i].size();
        }
        Ok(Self {
            rows_per_page,
            component_offsets: component_offsets.into_boxed_slice(),
        })
    }

    pub fn rows_per_page(&self) -> usize {
        self.rows_per_page
    }

    pub fn entity_offset(&self) -> usize {
        HEADER_SIZE
    }

    pub fn component_offset(&self, index: usize) -> usize {
        self.component_offsets[index]
    }
}

pub const FORMAT_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    Table = 1,
    Sparse = 2,
    Metadata = 3,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PageHeaderError {
    #[error("unknown page kind {0}")]
    UnknownKind(u8),
    #[error("unsupported page format version {0}")]
    UnsupportedVersion(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageHeader {
    pub lsn: u64,
    pub checksum: u32,
    pub archetype_id: u32,
    pub row_count: u16,
    pub kind: PageKind,
    pub version: u8,
}

impl PageHeader {
    pub fn read(page: &[u8; PAGE_SIZE]) -> Result<Self, PageHeaderError> {
        let lsn = u64::from_le_bytes(page[0..8].try_into().expect("slice is 8 bytes"));
        let checksum = u32::from_le_bytes(page[8..12].try_into().expect("slice is 4 bytes"));
        let archetype_id = u32::from_le_bytes(page[12..16].try_into().expect("slice is 4 bytes"));
        let row_count = u16::from_le_bytes(page[16..18].try_into().expect("slice is 2 bytes"));
        let kind = match page[18] {
            1 => PageKind::Table,
            2 => PageKind::Sparse,
            3 => PageKind::Metadata,
            _ => return Err(PageHeaderError::UnknownKind(page[18])),
        };
        let version = page[19];
        if version != FORMAT_VERSION {
            return Err(PageHeaderError::UnsupportedVersion(version));
        };
        Ok(Self {
            lsn,
            checksum,
            archetype_id,
            row_count,
            kind,
            version,
        })
    }

    pub fn write(&self, page: &mut [u8; PAGE_SIZE]) {
        page[0..8].copy_from_slice(&self.lsn.to_le_bytes());
        page[8..12].copy_from_slice(&self.checksum.to_le_bytes());
        page[12..16].copy_from_slice(&self.archetype_id.to_le_bytes());
        page[16..18].copy_from_slice(&self.row_count.to_le_bytes());
        page[18] = self.kind as u8;
        page[19] = self.version;
        page[20..24].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    /* AI GENERATED TEST */
    #[test]
    fn page_file_offset_is_computed_in_u64() {
        assert_eq!(PageId::new(0).file_offset(), 0);
        assert_eq!(PageId::new(1).file_offset(), 8192);
        assert_eq!(PageId::new(600_000).file_offset(), 600_000 * 8192);
        assert_eq!(
            PageId::new(u32::MAX).file_offset(),
            u64::from(u32::MAX) * 8192
        );
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).expect("valid layout")
    }

    /// Asserts that every minipage is aligned, fits in the page, and overlaps no other.
    fn assert_minipages_valid(components: &[Layout], page: &PageLayout) {
        let rows = page.rows_per_page();
        let mut ranges: Vec<(usize, usize)> = components
            .iter()
            .enumerate()
            .filter(|(_, component)| component.size() > 0)
            .map(|(index, component)| {
                let start = page.component_offset(index);
                assert_eq!(start % component.align(), 0, "component {index} misaligned");
                (start, start + rows * component.size())
            })
            .collect();
        ranges.push((
            page.entity_offset(),
            page.entity_offset() + rows * ENTITY_SIZE,
        ));
        ranges.sort_unstable();

        assert!(ranges[0].0 >= HEADER_SIZE, "minipage overlaps the header");
        assert!(
            ranges.last().expect("entity range").1 <= PAGE_SIZE,
            "minipage past page end"
        );
        for pair in ranges.windows(2) {
            assert!(
                pair[0].1 <= pair[1].0,
                "minipages {:?} and {:?} overlap",
                pair[0],
                pair[1]
            );
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn archetype_without_components_holds_only_entity_ids() {
        let page = PageLayout::compute(&[]).expect("valid layout");

        assert_eq!(page.entity_offset(), HEADER_SIZE);
        assert_eq!(
            page.rows_per_page(),
            (PAGE_SIZE - HEADER_SIZE) / ENTITY_SIZE
        );
    }

    /* AI GENERATED TEST */
    #[test]
    fn rows_per_page_divides_the_space_after_the_header_by_the_row_size() {
        // 80 bytes of components plus the 8-byte entity ID: 8168 / 88 = 92.
        let components = [layout(24, 8), layout(32, 8), layout(8, 4), layout(16, 8)];

        let page = PageLayout::compute(&components).expect("valid layout");

        assert_eq!(page.rows_per_page(), 92);
        assert_minipages_valid(&components, &page);
    }

    /* AI GENERATED TEST */
    #[test]
    fn minipages_are_ordered_by_alignment_largest_first() {
        // Declared (1-aligned, 8-aligned, 2-aligned). N = 8168 / (8 + 11) = 429.
        // Physical order: entities, then the 8-aligned, then the 2-aligned, then the 1-aligned.
        let components = [layout(1, 1), layout(8, 8), layout(2, 2)];

        let page = PageLayout::compute(&components).expect("valid layout");

        assert_eq!(page.rows_per_page(), 429);
        assert_eq!(page.entity_offset(), 24);
        assert_eq!(page.component_offset(1), 24 + 429 * 8);
        assert_eq!(page.component_offset(2), 24 + 429 * 8 + 429 * 8);
        assert_eq!(page.component_offset(0), 24 + 429 * 8 + 429 * 8 + 429 * 2);
        assert_minipages_valid(&components, &page);
    }

    /* AI GENERATED TEST */
    #[test]
    fn equal_alignments_keep_the_callers_order() {
        let components = [layout(4, 4), layout(12, 4), layout(4, 4)];

        let page = PageLayout::compute(&components).expect("valid layout");

        assert!(page.component_offset(0) < page.component_offset(1));
        assert!(page.component_offset(1) < page.component_offset(2));
        assert_minipages_valid(&components, &page);
    }

    /* AI GENERATED TEST */
    #[test]
    fn tags_take_no_space() {
        let without_tag = PageLayout::compute(&[layout(16, 8)]).expect("valid layout");
        let with_tags =
            PageLayout::compute(&[layout(0, 1), layout(16, 8), layout(0, 1)]).expect("valid");

        assert_eq!(with_tags.rows_per_page(), without_tag.rows_per_page());
        assert_eq!(
            with_tags.component_offset(1),
            without_tag.component_offset(0)
        );
    }

    /* AI GENERATED TEST */
    #[test]
    fn many_layouts_produce_valid_minipages() {
        let cases: [&[Layout]; 5] = [
            &[layout(8, 8)],
            &[layout(1, 1), layout(1, 1), layout(1, 1)],
            &[
                layout(2, 2),
                layout(16, 8),
                layout(4, 4),
                layout(1, 1),
                layout(0, 1),
            ],
            &[layout(100, 4), layout(6, 2)],
            &[layout(MAX_ROW_SIZE, 8)],
        ];

        for components in cases {
            let page = PageLayout::compute(components).expect("valid layout");
            assert!(page.rows_per_page() >= 1);
            assert_minipages_valid(components, &page);
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn row_at_the_size_limit_is_accepted() {
        // 2048 bytes of data plus the entity ID: 8168 / 2056 = 3 rows per page.
        let page = PageLayout::compute(&[layout(1024, 8), layout(1024, 8)]).expect("valid");

        assert_eq!(page.rows_per_page(), 3);
    }

    /* AI GENERATED TEST */
    #[test]
    fn row_over_the_size_limit_is_rejected() {
        let result = PageLayout::compute(&[layout(1024, 8), layout(1025, 1)]);

        assert_eq!(
            result,
            Err(PageLayoutError::RowTooLarge {
                size: 2049,
                max: MAX_ROW_SIZE
            })
        );
    }

    fn sample_header() -> PageHeader {
        PageHeader {
            lsn: 0x0102_0304_0506_0708,
            checksum: 0x1122_3344,
            archetype_id: 0xA1B2_C3D4,
            row_count: 0x5566,
            kind: PageKind::Table,
            version: FORMAT_VERSION,
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn header_write_then_read_round_trips() {
        let mut page = [0u8; PAGE_SIZE];
        let header = sample_header();

        header.write(&mut page);

        assert_eq!(PageHeader::read(&page), Ok(header));
    }

    /* AI GENERATED TEST */
    #[test]
    fn header_fields_are_little_endian_at_their_documented_offsets() {
        let mut page = [0u8; PAGE_SIZE];

        sample_header().write(&mut page);

        assert_eq!(page[0..8], 0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(page[8..12], 0x1122_3344u32.to_le_bytes());
        assert_eq!(page[12..16], 0xA1B2_C3D4u32.to_le_bytes());
        assert_eq!(page[16..18], 0x5566u16.to_le_bytes());
        assert_eq!(page[18], PageKind::Table as u8);
        assert_eq!(page[19], FORMAT_VERSION);
        assert_eq!(page[20..24], [0; 4]);
    }

    /* AI GENERATED TEST */
    #[test]
    fn header_write_zeroes_reserved_bytes_and_leaves_the_rest_untouched() {
        let mut page = [0xAB; PAGE_SIZE];

        sample_header().write(&mut page);

        assert_eq!(page[20..24], [0; 4]);
        assert!(page[HEADER_SIZE..].iter().all(|&byte| byte == 0xAB));
    }

    /* AI GENERATED TEST */
    #[test]
    fn every_page_kind_round_trips() {
        for kind in [PageKind::Table, PageKind::Sparse, PageKind::Metadata] {
            let mut page = [0u8; PAGE_SIZE];
            let header = PageHeader {
                kind,
                ..sample_header()
            };

            header.write(&mut page);

            assert_eq!(PageHeader::read(&page).map(|read| read.kind), Ok(kind));
        }
    }

    /* AI GENERATED TEST */
    #[test]
    fn reading_a_never_written_page_fails() {
        let page = [0u8; PAGE_SIZE];

        assert!(PageHeader::read(&page).is_err());
    }

    /* AI GENERATED TEST */
    #[test]
    fn reading_an_unknown_kind_fails() {
        let mut page = [0u8; PAGE_SIZE];
        sample_header().write(&mut page);
        page[18] = 0xEE;

        assert_eq!(
            PageHeader::read(&page),
            Err(PageHeaderError::UnknownKind(0xEE))
        );
    }

    /* AI GENERATED TEST */
    #[test]
    fn reading_an_unsupported_version_fails() {
        let mut page = [0u8; PAGE_SIZE];
        sample_header().write(&mut page);
        page[19] = 99;

        assert_eq!(
            PageHeader::read(&page),
            Err(PageHeaderError::UnsupportedVersion(99))
        );
    }
}
