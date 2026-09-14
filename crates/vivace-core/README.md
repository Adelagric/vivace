# vivace-core

The lower layer of [vivace](https://github.com/Adelagric/vivace), a Composer-compatible installer in Rust: composer.json/composer.lock reading, Composer's content hash, platform checks, dist fetching with Composer's cache and auth, a content-addressed store cloned into vendor/, and the emulated installers. Byte-identical output with Composer 2.10.3 is checked by the harness in the repository.

License: MIT or Apache-2.0. The code ported from Composer and its libraries is © Nils Adermann, Jordi Boggiano and the Composer project (MIT); see NOTICE.md in the repository.
