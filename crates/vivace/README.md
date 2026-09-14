# vivace

`composer install`, `update`, `require`, `remove` and `dump-autoload` reimplemented in Rust, writing the same vendor/, composer.json edits and composer.lock as Composer 2.10.3, byte for byte, without running PHP. This crate is the command layer: a library (`vivace::run(args)`) with a thin binary on top. Documentation, harness and benchmarks: https://github.com/Adelagric/vivace

License: MIT or Apache-2.0. The code ported from Composer and its libraries is © Nils Adermann, Jordi Boggiano and the Composer project (MIT); see NOTICE.md in the repository.
