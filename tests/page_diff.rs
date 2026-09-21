use inkdb::storage::page::{BTreePage, BTreePageType};
use inkdb::storage::page_old;

struct Rng(u64);
impl Rng {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

fn snap_new(p: &BTreePage<&mut [u8]>) -> (u8, u16, Option<u32>, Vec<Vec<u8>>) {
    let n = p.no_of_cells().unwrap();
    let mut cells = Vec::new();
    for i in 0..n {
        cells.push(p.cell_bytes_as_ref(i).unwrap().to_vec());
    }
    cells.sort();
    (
        p.page_type().unwrap() as u8,
        n,
        p.right_most_ptr().unwrap(),
        cells,
    )
}

fn snap_old(
    p: &page_old::BTreePageMut,
) -> (u8, u16, Option<u32>, Vec<Vec<u8>>) {
    let n = p.header.no_of_cells;
    let mut cells = Vec::new();
    for i in 0..n {
        cells.push(p.cell_bytes_as_ref(i).unwrap().to_vec());
    }
    cells.sort();
    (
        p.header.page_kind as u8,
        n,
        p.header.right_most_ptr,
        cells,
    )
}

fn mkcell(rng: &mut Rng) -> Vec<u8> {
    use inkdb::record::Value;
    use inkdb::record::tuple::Tuple;
    use inkdb::storage::cell::Encode;
    let a = rng.next(100000) as i64;
    let b = rng.next(100000) as i64;
    let rec = if rng.next(2) == 0 {
        vec![Value::Integer(a), Value::Integer(b)]
    } else {
        let tlen = rng.next(32);
        let s: String = (0..tlen).map(|j| (b'a' + ((rng.next(26) + j) % 26) as u8) as char).collect();
        vec![Value::Text(s.into()), Value::Integer(b)]
    };
    Encode::encode_index_leaf_cell(Tuple::serialize(&rec))
}

fn check_page(buf: &[u8], page_no: u32, usable: usize) {
    use inkdb::storage::page::{BTreePage, BTreePageType};
    let p = BTreePage::new(page_no, buf.len(), usable, buf).unwrap();
    let t = p.page_type().unwrap();
    let n = p.no_of_cells().unwrap() as usize;
    let cca = p.cell_content_area().unwrap() as usize;
    let hs = p.header_size().unwrap() as usize;
    assert!(hs + 2 * n <= cca, "array overruns gap");
    assert!(cca <= usable, "cca beyond usable");
    let mut spans = Vec::new();
    for i in 0..n {
        let ptr = p.cell_ptr(i as u16).unwrap() as usize;
        assert!((cca..usable).contains(&ptr), "ptr {} out of content area", ptr);
        let sp = p.cell_span(ptr as u16).unwrap();
        assert!(sp.start >= cca && sp.end <= usable && sp.start < sp.end, "bad span {:?}", sp);
        spans.push(sp);
    }
    spans.sort_by_key(|r| r.start);
    for w in spans.windows(2) {
        assert!(w[0].end <= w[1].start, "overlapping cells");
    }
    if t.is_interior() {
        assert!(p.right_most_ptr().unwrap().is_some(), "interior without rmp");
    }
    let _ = t;
}

fn ptrs_of(p: &BTreePage<&mut [u8]>) -> Vec<u16> {
    let n = p.no_of_cells().unwrap();
    (0..n).map(|i| p.cell_ptr(i).unwrap()).collect()
}

fn run_seed(seed: u64) {
    let ps = 512usize;
    let mut abuf = vec![0u8; ps];
    let mut bbuf = vec![0u8; ps];
    {
        let mut a =
            BTreePage::new_from_raw_bytes(7, BTreePageType::LeafIndex, &mut abuf[..], ps, ps)
                .unwrap();
        let mut b = page_old::BTreePageMut::new_from_raw_bytes(
            7,
            page_old::BTreePageType::LeafIndex,
            &mut bbuf[..],
            ps,
            ps,
        );
        let sa = snap_new(&a);
        let sb = snap_old(&b);
        assert_eq!(sa, sb, "init drift");
        let _ = (&mut a, &mut b);
    }
    check_page(&abuf, 7, ps);
    assert_eq!(abuf, bbuf, "init buffer drift");
    let mut rng = Rng(seed);
    let mut log: Vec<String> = Vec::new();
    for step in 0..3000 {
        eprintln!("STEP {}", step);
        {
            let mut a = BTreePage::new(7, ps, ps, &mut abuf[..]).unwrap();
            let mut b = page_old::BTreePageMut::new(7, &mut bbuf[..], ps, ps).unwrap();
            let n = a.no_of_cells().unwrap();
            assert_eq!(n, b.header.no_of_cells, "count drift at step {}", step);
        match rng.next(100) {
            0..=59 => {
                let content = mkcell(&mut rng);
                let len = content.len();
                let idx = rng.next(n as usize + 1) as u16;
                let ra = a.insert_cell(&content, idx);
                let rb = b.insert_cell(&content, idx);
                if format!("{:?}", ra) != format!("{:?}", rb) {
                    eprintln!("DIVERGE-RET step={} n={} idx={} len={} old_n={}", step, n, idx, len, b.header.no_of_cells);
                    eprintln!("new hdr={:x?}", &abuf[..16]);
                    eprintln!("old hdr={:x?}", &bbuf[..16]);
                    eprintln!("new free={} frag={} cca={}", a.first_freeblock().unwrap(), abuf[7], a.cell_content_area().unwrap());
                    eprintln!("old free={} frag={} cca={}", b.header.first_freeblock, b.header.frag_cnt, b.header.cell_content_area);
                    let sa = snap_new(&a);
                    let sb = snap_old(&b);
                    eprintln!("DIVERGE step={} n={} idx={} len={}", step, n, idx, len);
                    eprintln!("new snap={:?}", sa);
                    eprintln!("old snap={:?}", sb);
                    drop(sa);
                    drop(sb);
                }
                assert_eq!(
                    format!("{:?}", ra),
                    format!("{:?}", rb),
                    "insert result drift at step {} (n={}, idx={}, len={})",
                    step,
                    n,
                    idx,
                    len
                );
                log.push(format!("insert idx={} len={}", idx, len));
            }
            60..=79 => {
                if n == 0 {
                    continue;
                }
                let idx = rng.next(n as usize) as u16;
                a.remove_cell(idx).unwrap();
                b.remove_cell(idx).unwrap();
                log.push(format!("remove idx={}", idx));
            }
            80..=89 => {
                if n == 0 {
                    continue;
                }
                let content = mkcell(&mut rng);
                let len = content.len();
                let idx = rng.next(n as usize) as u16;
                let ra = a.replace_cell(idx, &content);
                let rb = b.replace_cell(idx, &content);
                assert_eq!(
                    format!("{:?}", ra),
                    format!("{:?}", rb),
                    "replace result drift at step {}",
                    step
                );
                log.push(format!("replace idx={} len={}", idx, len));
            }
            _ => {
                a.reset_for_rebuild().unwrap();
                b.reset_for_rebuild();
                log.push("reset".to_string());
            }
        }
        let sa = snap_new(&a);
        let sb = snap_old(&b);
        if sa != sb {
            eprintln!("step {} ptrs_new={:?} ptrs_old={:?}", step, ptrs_of(&a), b.cell_pointers);
            eprintln!("hdr_new={:x?} hdr_old={:x?}", &abuf[..16], &bbuf[..16]);
        }
        assert_eq!(sa, sb, "state drift at step {}\nlog: {:?}", step, log);
        }
        check_page(&abuf, 7, ps);
    }
}

#[test]
fn page_old_new_differential() {
    for seed in [1, 7, 42, 1234] {
        run_seed(seed);
    }
}
