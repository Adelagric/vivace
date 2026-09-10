fn main() {
    let root = std::env::args().nth(1).unwrap();
    let files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("php"))
        .collect();
    let t = std::time::Instant::now();
    let threads = 14;
    let chunks: Vec<&[std::path::PathBuf]> = files.chunks(files.len().div_ceil(threads)).collect();
    let total: usize = std::thread::scope(|s| {
        chunks
            .iter()
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|f| std::fs::read(f).map(|b| b.len()).unwrap_or(0))
                        .sum::<usize>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum()
    });
    println!(
        "lecture parallèle: {} Mo en {:?}",
        total / 1_000_000,
        t.elapsed()
    );
}
