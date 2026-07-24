#[macro_export]
macro_rules! sqlite_assert_all {
    ($($assert_expr: expr), * $(,)?) => {
        $(assert!($assert_expr, "Assertion Failed: {}",stringify!($assert_expr));)*
    };
}

#[macro_export]
// seek current
macro_rules! seek_c {
    ($x: expr) => {
        std::io::SeekFrom::Current($x as u64)
    };
}

//seek start
#[macro_export]
macro_rules! seek_s {
    ($x: expr) => {
        std::io::SeekFrom::Start($x as u64)
    };
}

#[macro_export]
macro_rules! to_int {
    (u8, $x:expr) => {{
        u8::from_be_bytes($x)
    }};
    (u16, $x:expr) => {{
        u16::from_be_bytes($x)
    }};
    (u32, $x:expr) => {{
        u32::from_be_bytes($x)
    }};
    (u64, $x:expr) => {{
        u64::from_be_bytes($x)
    }};
}
