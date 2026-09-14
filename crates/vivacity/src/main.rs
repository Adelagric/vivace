//! The binary: the command line handed to the library.

fn main() {
    std::process::exit(vivacity::run(std::env::args_os()));
}
