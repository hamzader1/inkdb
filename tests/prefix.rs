use inkdb::storage::page::{BTreePage, BTreePageType};
use inkdb::storage::page_old;

fn run_prefix(k: usize) -> bool {
    let ps = 512usize;
    let mut abuf = vec![0u8; ps];
    let mut bbuf = vec![0u8; ps];
    let mut rng = PrefixRng(1);
    let mut a = BTreePage::new_from_raw_bytes(7, BTreePageType::LeafIndex, &mut abuf[..], ps, ps).unwrap();
    let mut b = page_old::BTreePageMut::new_from_raw_bytes(7, page_old::BTreePageType::LeafIndex, &mut bbuf[..], ps, ps);
    drop(a);
    drop(b);
    let mut log: Vec<String> = Vec::new();
    for step in 0..k {
        let mut a = BTreePage::new(7, ps, ps, &mut abuf[..]).unwrap();
        let mut b = page_old::BTreePageMut::new(7, &mut bbuf[..], ps, ps).unwrap();
        let n = a.no_of_cells().unwrap();
        match rng.next(100) {
            0..=59 => {
                let content = mkcell(&mut rng);
                let idx = rng.next(n as usize + 1) as u16;
                let ra = a.insert_cell(&content, idx);
                let rb = b.insert_cell(&content, idx);
                assert_eq!(format!("{:?}", ra), format!("{:?}", rb));
                log.push(format!("insert idx={} len={}", idx, content.len()));
            }
            60..=79 => {
                if n == 0 { continue; }
                let idx = rng.next(n as usize) as u16;
                a.remove_cell(idx).unwrap();
                b.remove_cell(idx).unwrap();
                log.push(format!("remove idx={}", idx));
            }
            80..=89 => {
                if n == 0 { continue; }
                let content = mkcell(&mut rng);
                let idx = rng.next(n as usize) as u16;
                let ra = a.replace_cell(idx, &content);
                let rb = b.replace_cell(idx, &content);
                assert_eq!(format!("{:?}", ra), format!("{:?}", rb));
                log.push(format!("replace idx={} len={}", idx, content.len()));
            }
            _ => {
                a.reset_for_rebuild().unwrap();
                b.reset_for_rebuild();
                log.push("reset".to_string());
            }
        }
    }
    abuf == bbuf
}

struct PrefixRng(u64);
impl PrefixRng {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

fn mkcell(rng: &mut PrefixRng) -> Vec<u8> {
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

#[test]
fn bisect_prefix() {
    // NOTE: mkcell/Rng must mirror page_diff seed 1 exactly; find first diverging k
    for k in [5, 8, 10, 12, 14, 16, 17] {
        eprintln!("k={} equal={}", k, run_prefix(k));
    }
}
