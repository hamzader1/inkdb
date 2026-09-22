//! Randomized page-operation fuzz against a trivial model.
//!
//! Replaces the old-vs-new differential tests (the old page module is
//! gone): random insert/remove/replace/reset sequences run against a
//! real page while a plain `Vec` mirrors every op. After each step the
//! page must contain exactly the model's cells (order-insensitive),
//! satisfy the structural invariants, and report honest fullness.
//! This is what guards the page layer the remake builds on.

use inkdb::storage::page::{BTreePage, BTreePageType};

struct Rng(u64);
impl Rng {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
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
        let s: String = (0..tlen)
            .map(|j| (b'a' + ((rng.next(26) + j) % 26) as u8) as char)
            .collect();
        vec![Value::Text(s.into()), Value::Integer(b)]
    };
    Encode::encode_index_leaf_cell(Tuple::serialize(&rec))
}

/// Structural invariants straight from the file format: pointer array
/// inside the gap, every body inside the content area, no overlaps, an
/// interior always keeps its right-most pointer.
fn check_page(buf: &[u8], page_no: u32, usable: usize) {
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
        assert!(
            (cca..usable).contains(&ptr),
            "ptr {ptr} out of content area"
        );
        let sp = p.cell_span(ptr as u16).unwrap();
        assert!(
            sp.start >= cca && sp.end <= usable && sp.start < sp.end,
            "bad span {sp:?}"
        );
        spans.push(sp);
    }
    spans.sort_by_key(|r| r.start);
    for w in spans.windows(2) {
        assert!(w[0].end <= w[1].start, "overlapping cells");
    }
    if t.is_interior() {
        assert!(
            p.right_most_ptr().unwrap().is_some(),
            "interior without rmp"
        );
    }
}

fn page_cells(buf: &mut [u8], page_no: u32, usable: usize) -> Vec<Vec<u8>> {
    let p = BTreePage::new(page_no, buf.len(), usable, buf).unwrap();
    let n = p.no_of_cells().unwrap();
    let mut cells: Vec<Vec<u8>> = (0..n)
        .map(|i| p.cell_bytes_as_ref(i).unwrap().to_vec())
        .collect();
    cells.sort();
    cells
}

fn run_seed(seed: u64) {
    let ps = 512usize;
    let mut buf = vec![0u8; ps];
    BTreePage::new_from_raw_bytes(7, BTreePageType::LeafIndex, &mut buf[..], ps, ps).unwrap();
    let mut model: Vec<Vec<u8>> = Vec::new();
    let mut rng = Rng(seed);
    let mut log: Vec<String> = Vec::new();
    let fail = |step: usize, msg: String, log: &[String]| -> ! {
        panic!("seed {seed} step {step}: {msg}\nlog: {log:?}");
    };
    for step in 0..2000 {
        let before_n = model.len();
        match rng.next(100) {
            0..=59 => {
                let content = mkcell(&mut rng);
                let idx = rng.next(before_n + 1);
                let mut p = BTreePage::new(7, ps, ps, &mut buf[..]).unwrap();
                match p.insert_cell(&content, idx as u16) {
                    Ok(state) => {
                        let inserted = format!("{state:?}") == "Inserted";
                        if inserted {
                            model.insert(idx, content.clone());
                        }
                        log.push(format!(
                            "insert idx={idx} len={} -> {state:?}",
                            content.len()
                        ));
                    }
                    Err(e) => fail(step, format!("insert err {e:?}"), &log),
                }
                drop(p);
            }
            60..=79 => {
                if before_n == 0 {
                    continue;
                }
                let idx = rng.next(before_n);
                let mut p = BTreePage::new(7, ps, ps, &mut buf[..]).unwrap();
                if let Err(e) = p.remove_cell(idx as u16) {
                    fail(step, format!("remove err {e:?}"), &log);
                }
                drop(p);
                model.remove(idx);
                log.push(format!("remove idx={idx}"));
            }
            80..=89 => {
                if before_n == 0 {
                    continue;
                }
                let content = mkcell(&mut rng);
                let idx = rng.next(before_n);
                let snapshot = buf.clone();
                let outcome = {
                    let mut p = BTreePage::new(7, ps, ps, &mut buf[..]).unwrap();
                    match p.replace_cell(idx as u16, &content) {
                        Ok(state) => format!("{state:?}"),
                        Err(e) => fail(step, format!("replace err {e:?}"), &log),
                    }
                };
                if outcome == "Inserted" {
                    model[idx] = content.clone();
                } else {
                    // Atomicity contract: a refused replace must leave
                    // every byte exactly as it was.
                    assert_eq!(buf, snapshot, "refused replace mutated the page");
                }
                log.push(format!("replace idx={idx} -> {outcome}"));
            }
            _ => {
                let mut p = BTreePage::new(7, ps, ps, &mut buf[..]).unwrap();
                if let Err(e) = p.reset_for_rebuild() {
                    fail(step, format!("reset err {e:?}"), &log);
                }
                drop(p);
                model.clear();
                log.push("reset".to_string());
            }
        }
        let got = page_cells(&mut buf, 7, ps);
        let mut want = model.clone();
        want.sort();
        if got != want {
            fail(
                step,
                format!("content drift: page={} model={}", got.len(), want.len()),
                &log,
            );
        }
        check_page(&buf, 7, ps);
        if log.len() > 40 {
            log.drain(..20);
        }
    }
}

#[test]
fn page_ops_match_model() {
    for seed in [1, 7, 42] {
        run_seed(seed);
    }
}

#[test]
fn full_page_reports_honestly() {
    // Fill a page, then prove oversize work is refused, never half done.
    let ps = 512usize;
    let mut buf = vec![0u8; ps];
    BTreePage::new_from_raw_bytes(7, BTreePageType::LeafIndex, &mut buf[..], ps, ps).unwrap();
    let big = vec![0xAAu8; 200];
    let mut inserted = 0;
    loop {
        let mut p = BTreePage::new(7, ps, ps, &mut buf[..]).unwrap();
        match p.insert_cell(&big, 0) {
            Ok(state) => {
                if format!("{state:?}") != "Inserted" {
                    break;
                }
                inserted += 1;
            }
            Err(e) => panic!("insert err {e:?}"),
        }
        drop(p);
        if inserted > 100 {
            panic!("page never filled");
        }
    }
    assert!(inserted > 0);
    let snapshot = buf.clone();
    let outcome = {
        let mut p = BTreePage::new(7, ps, ps, &mut buf[..]).unwrap();
        match p.replace_cell(0, &vec![0xBBu8; 400]) {
            Ok(state) => format!("{state:?}"),
            Err(e) => panic!("replace err {e:?}"),
        }
    };
    assert_ne!(
        outcome, "Inserted",
        "oversize replace must not report success"
    );
    assert_eq!(buf, snapshot, "refused replace mutated the page");
    check_page(&buf, 7, ps);
}
