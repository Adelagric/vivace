//! Le binaire : la ligne de commande passée à la bibliothèque.

fn main() {
    std::process::exit(vivace::run(std::env::args_os()));
}
