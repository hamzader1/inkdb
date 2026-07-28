use super::page::PageNumber;

// This later would replace RawPage
pub struct RawPage {
    next: PageNumber,
    data: Vec<u8>,
}


// impl<'a> RawPage<'a> {
//     pub fn new<T: AsRef<[u8]> + ?Sized>(data: &'a T) -> Self {
//         Self {
//             data: data.as_ref(),
//         }
//     }

//     pub fn data(&'a self) -> &'a [u8] {
//         self.data
//     }
// }
