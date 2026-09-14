# Notices

vivacity is distributed under the MIT or Apache-2.0 license, at your option
(see LICENSE-MIT and LICENSE-APACHE). It reimplements the behaviour of
Composer, and parts of it are ports — function by function — of the
following software. Their source files are vendored in `docs/reference/`
as the reference each port is checked against (the copies come from the
Composer 2.10.3 phar, which strips comment headers; the license texts are
kept next to them). The copyright notices below apply to those files and
to the Rust code ported from them.

| Origin | License | Copyright | Vendored copy | Ported in |
|---|---|---|---|---|
| [Composer](https://github.com/composer/composer) 2.10.3 (`src/Composer/`) | MIT | Nils Adermann, Jordi Boggiano | `docs/reference/*.php`, `docs/reference/resolver/`, `docs/reference/policy/` (`LICENSE.composer`) | `vivacity-core`, `vivacity-resolver`, `vivacity-autoload`, `vivacity` |
| [composer/semver](https://github.com/composer/semver) | MIT | Composer | `docs/reference/resolver/semver-*.php` (`LICENSE.composer-semver`) | `vivacity-resolver` (`version`, `constraint`, `intervals`, `phpver`) |
| [composer/class-map-generator](https://github.com/composer/class-map-generator) | MIT | Composer | `docs/reference/cmg-*.php` (`LICENSE.composer-class-map-generator`) | `vivacity-autoload` |
| [composer/metadata-minifier](https://github.com/composer/metadata-minifier) | MIT | Composer | `docs/reference/resolver/MetadataMinifier.php` (`LICENSE.composer-metadata-minifier`) | `vivacity-resolver` (`loader`) |
| [composer/installers](https://github.com/composer/installers) 2.0.0–2.3.0 | MIT | Kyle Robinson Young | `docs/reference/installers/` (`LICENSE`) | `vivacity-core` (`installers`, tables in `assets/installers/`) |

The PHP sources under `docs/reference/` are not part of the compiled
crates; they are kept so that `harness/drift-reference.sh` can re-diff
them against the phar and name the port that must be re-read when
upstream moves.

Nothing is ported from GPL-licensed software. Releases 0.3.0 to 0.5.0
shipped an emulation of `drupal/core-composer-scaffold` (GPL-2.0-or-later)
ported from its source; it was removed in 0.6.0 for that reason, and those
releases are yanked on crates.io.
