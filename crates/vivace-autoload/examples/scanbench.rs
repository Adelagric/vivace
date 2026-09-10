//! Décomposition du coût du scan de classmap (M5). Usage : scanbench <vendor-dir>
use std::time::Instant;

fn main() {
    let root = std::env::args().nth(1).expect("usage: scanbench <dir>");
    let t = Instant::now();
    let files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root)
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("php"))
        .collect();
    println!(
        "walk (trié, symlinks suivis): {} fichiers en {:?}",
        files.len(),
        t.elapsed()
    );

    let t = Instant::now();
    let contents: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(f).unwrap_or_default())
        .collect();
    let bytes: usize = contents.iter().map(Vec::len).sum();
    println!("lecture: {} Mo en {:?}", bytes / 1_000_000, t.elapsed());

    let t = Instant::now();
    let mut n = 0usize;
    for f in &files {
        if std::fs::canonicalize(f).is_ok() {
            n += 1;
        }
    }
    println!("canonicalize par fichier: {n} en {:?}", t.elapsed());

    let finder = vivace_autoload::classmap::ClassFinder::new().expect("regex");
    let t = Instant::now();
    let mut classes = 0usize;
    for c in &contents {
        classes += finder.find_classes(c).expect("find").len();
    }
    println!(
        "find_classes séquentiel: {classes} classes en {:?}",
        t.elapsed()
    );

    let t = Instant::now();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let chunks: Vec<&[Vec<u8>]> = contents.chunks(contents.len().div_ceil(threads)).collect();
    let total: usize = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                s.spawn(move || {
                    let finder = vivace_autoload::classmap::ClassFinder::new().expect("regex");
                    chunk
                        .iter()
                        .map(|c| finder.find_classes(c).expect("find").len())
                        .sum::<usize>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("thread")).sum()
    });
    println!(
        "find_classes sur {threads} threads: {total} classes en {:?}",
        t.elapsed()
    );
}
