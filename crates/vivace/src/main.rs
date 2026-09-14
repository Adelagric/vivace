//! The binary: the command line handed to the library.

fn main() {
    std::process::exit(vivace::run(std::env::args_os()));
}
