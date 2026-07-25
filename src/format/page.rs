use crate::bytes::{read_u16_be, read_u32_be};
use crate::util::assert_one;
use crate::{bytes::read_u8, errors::SqliteDatabaseError};
use crate::{decode_varint, seek_c, seek_s};
use std::io::{Read, Seek, SeekFrom};

// pub const INTERIOR_INEDX_BTREE_PAGE: u8 = 0x02;
// pub const LEAF_INEDX_BTREE_PAGE: u8 = 0x0a;
//
// pub const INTERIOR_TABLE_BTREE_PAGE: u8 = 0x05;
// pub const LEFT_TABLE_BTREE_PAGE: u8 = 0x0d;

pub const BTREE_TYPE_PAGE_OFFSET: u8 = 0;
pub const BTREE_TYPE_PAGE_SIZE: u8 = 1;

pub const FIRST_FREEBLOCK_OFFSET: usize = 1;
pub const FIRST_FREEBLOCK_SIZE: usize = 2;

pub const CELL_COUNT_OFFSET: usize = 3;
pub const CELL_COUNT_SIZE: usize = 2;

pub const CELL_CONTENT_AREA_OFFSET: usize = 5;
pub const CELL_CONTENT_AREA_SIZE: usize = 2;

pub const FRAGMENTED_FREE_BYTES_OFFSET: usize = 7;
pub const FRAGMENTED_FREE_BYTES_SIZE: usize = 1;

pub const RIGHT_MOST_POINTER_OFFSET: usize = 8;
pub const RIGHT_MOST_POINTER_SIZE: usize = 4;

#[derive(Debug)]
struct PageNo(u32);

#[derive(Debug)]
struct CellPointer(u16); //

// #[derive(Debug)]
// enum CellPointers {
//     Leaf(Vec<u16>),
//     Interior(Vec<u32>),
// }

#[derive(Debug)]
pub struct BTreePage {
    header: BTreePageHeader,
    cell_pointers: Vec<CellPointer>,
}
impl BTreePage {
    pub fn parse<R: Read + Seek>(r: &mut R) -> Result<Self, SqliteDatabaseError> {
        let (header, cell_pointers) = BTreePageHeader::parse_header(r)?;
        Ok(Self {
            header,
            cell_pointers,
        })
    }
}

#[derive(Debug)]
pub enum BTreePageType {
    InteriorIndex = 0x02,
    LeafIndex = 0x0a,
    InteriorTable = 0x05,
    LeafTable = 0x0d,
}
impl BTreePageType {
    fn get(kind: u8) -> Option<Self> {
        match kind {
            0x0a => Some(Self::LeafIndex),
            0x02 => Some(Self::InteriorIndex),
            0x0d => Some(Self::LeafTable),
            0x05 => Some(Self::InteriorTable),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct BTreePageHeader {
    page_kind: BTreePageType,
    first_freeblock: u16,
    no_of_cells: u16,
    cell_content_area: u16,
    frag_cnt: u8,
    right_most_ptr: Option<PageNo>,
}
impl BTreePageHeader {
    fn parse_header<R: Read + Seek>(
        r: &mut R,
    ) -> Result<(Self, Vec<CellPointer>), SqliteDatabaseError> {
        r.seek(seek_s!(BTREE_TYPE_PAGE_OFFSET));
        let p_kind = BTreePageType::get(read_u8(r)?).unwrap();
        match p_kind {
            BTreePageType::LeafTable => {
                return Self::parse_page(r, BTreePageType::LeafTable, false)
            }
            BTreePageType::InteriorTable => {
                return Self::parse_page(r, BTreePageType::InteriorTable, true)
            }
            _ => todo!(),
        };
    }
    // fn parse_leaf_table<R: Read + Seek>(
    //     r: &mut R,
    // ) -> Result<(Self, Vec<CellPointer>), SqliteDatabaseError> {
    //     let first_freeblock: u16 = read_u16_be(r)?;
    //     let no_of_cells: u16 = read_u16_be(r)?;
    //     let cell_content_area: u16 = read_u16_be(r)?;
    //     let frag_cnt: u8 = read_u8(r)?;
    //     let page_header = Self {
    //         page_kind: BTreePageType::LeafTable,
    //         first_freeblock,
    //         no_of_cells,
    //         cell_content_area,
    //         frag_cnt,
    //         right_most_ptr: None,
    //     };
    //     let mut cell_pointers = Vec::<CellPointer>::new();
    //     for _ in 0..no_of_cells {
    //         let cell_pointer = CellPointer(read_u16_be(r)?);
    //         cell_pointers.push(cell_pointer);
    //     }

    //     // TODO:
    //     // leaf_header.validate_table()?;
    //     Ok((page_header, cell_pointers))
    // }
    // fn parse_interior_table<R: Read + Seek>(
    //     r: &mut R,
    // ) -> Result<(Self, Vec<CellPointer>), SqliteDatabaseError> {
    //     let first_freeblock: u16 = read_u16_be(r)?;
    //     let no_of_cells: u16 = read_u16_be(r)?;
    //     let cell_content_area: u16 = read_u16_be(r)?;
    //     let frag_cnt: u8 = read_u8(r)?;
    //     let right_most_ptr: u32 = read_u32_be(r)?;
    //     let page_header = Self {
    //         page_kind: BTreePageType::InteriorTable,
    //         first_freeblock,
    //         no_of_cells,
    //         cell_content_area,
    //         frag_cnt,
    //         right_most_ptr: Some(PageNo(right_most_ptr)),
    //     };

    //     let mut cell_pointers = Vec::<CellPointer>::new();
    //     for _ in 0..no_of_cells {
    //         let cell_pointer = CellPointer(read_u16_be(r)?);
    //         cell_pointers.push(cell_pointer);
    //     }

    //     Ok((page_header, cell_pointers))
    // }

    fn parse_page<R: Read + Seek>(
        r: &mut R,
        page_kind: BTreePageType,
        is_interior: bool,
    ) -> Result<(Self, Vec<CellPointer>), SqliteDatabaseError> {
        r.seek(SeekFrom::Start(BTREE_TYPE_PAGE_OFFSET as u64));
        let first_freeblock: u16 = read_u16_be(r)?;
        let no_of_cells: u16 = read_u16_be(r)?;
        let cell_content_area: u16 = read_u16_be(r)?;
        let frag_cnt: u8 = read_u8(r)?;
        let right_most_ptr: Option<PageNo> = if is_interior {
            Some(PageNo(read_u32_be(r)?))
        } else {
            None
        };
        let page_header = Self {
            page_kind,
            first_freeblock,
            no_of_cells,
            cell_content_area,
            frag_cnt,
            right_most_ptr,
        };

        let mut cell_pointers = Vec::<CellPointer>::new();
        for _ in 0..no_of_cells {
            let cell_pointer = CellPointer(read_u16_be(r)?);
            cell_pointers.push(cell_pointer);
        }

        Ok((page_header, cell_pointers))
    }
    // TODO:
    // fn validate_table(&self) -> Result<(), SqliteDatabaseError> {
    //     Ok(())
    // }
}
