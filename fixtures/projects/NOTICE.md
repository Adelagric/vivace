# Fixture projects

Frozen application skeletons used by the differential harness. They are
copied verbatim (vendor/, node_modules/, var/ and .env excluded) from the
upstream `create-project` templates, together with the `composer.lock` that
was resolved on 2026-09-09/10; freezing them keeps the harness deterministic
and independent of upstream churn. They are test data, not part of vivacity.

| fixture | upstream | license |
|---|---|---|
| laravel | laravel/laravel | MIT |
| symfony | symfony/demo (symfony/symfony-demo) | MIT (see its LICENSE) |
| sylius | sylius/sylius-standard | MIT |
| rector | rectorphp/rector-src | MIT |
| wordpress | (own manifest, see below) | MIT for the manifest; packages have their own licenses |
| drupal | drupal/recommended-project 11.4.6 | GPL-2.0-or-later (composer.json/lock only; Drupal core and modules are downloaded, never committed) |

`wordpress` is not a copy of an upstream template: it is a small manifest
written for the harness (`composer.json` + the lock resolved on 2026-09-10)
that exercises `composer/installers` — Bedrock-style `installer-paths` for
plugins and mu-plugins, default `wp-content/themes/` for themes, a plugin with
a PSR-4 autoload (`roots/soil`, so the boot can `class_exists` a class living
outside vendor/), a `package` repository entry with a `bin` (proxy outside
vendor/), and one ordinary library. The WordPress plugins and theme
(GPL-2.0-or-later) are downloaded by `fixtures/make.sh` from wordpress.org;
nothing from them is committed here. Its boot check is `class_exists('Roots\Soil\Options')`.

`rector` is a library, not an application template, and upstream does not
commit a `composer.lock`: the skeleton holds `composer.json`, the lock
resolved on 2026-09-10 with `composer update --no-install`, and only the
autoload entry points (the `files` entries verbatim, the psr-4/classmap
directories as empty placeholders). It covers `dev-main` packages flagged
`default-branch`, `phpstan/extension-installer` + `rector/extension-installer`
and `platform-check: false`. Its boot check is `vendor/bin/phpstan --version`.

`drupal` is the `drupal/recommended-project` template as `create-project`
leaves it (composer.json, the lock resolved on 2026-09-11, LICENSE.txt), plus
`web/example.gitignore` copied to `.gitignore` the way the template's own
instructions suggest. It locks five plugins: composer/installers and
symfony/runtime (emulated), drupal/core-project-message and
drupal/core-recipe-unpack (inert at install time), and
drupal/core-composer-scaffold, which vivacity does not emulate (GPL source):
the fixture proves the Composer fallback path and `harness/transitions.sh`
the refusal before any write. Boot check: `vendor/bin/dr --version`.

`solver-*` are four small manifests written for the resolver oracle, with
no lock and no vendor: `solver-backtrack` (phpunit `^10 || ^11 || ^12`
against `sebastian/version ^4`), `solver-conflict` (monolog 3 with
psr/log 1, deliberately unsolvable), `solver-aliases` (a `dev-master as
3.99.0` root alias, a `7.4.x-dev` branch, `minimum-stability: dev` with
`prefer-stable`), `solver-providers` (virtual packages with several
providers, `symfony/polyfill-mbstring`). Their Packagist snapshots live in
`fixtures/registry/solver-*.tar.gz`; `harness/update.sh` does not run them
(they have nothing to install), `tests/oracle_pool.rs` does.

`path-repos` is a manifest written for the harness (MIT, like the harness):
six `acme/*` packages served by three `path` repositories — a glob
(`packages/*`), a brace list with `symlink: false` and a `versions`
override (`libs/{beta,epsilon}`), a plain `./src-zeta` with `reference:
none`. `fixtures/path-repos.sh` materialises what git cannot store: the
nested repositories of `gamma` (a `feature-x` branch off `main`) and
`epsilon` (its HEAD is the dist reference), the symbolic links (into a
file, an empty directory, a non-empty directory, dangling, outside the
package), the `0600`/`0755` modes, the empty directory, the `.git` *file*
of a sub-directory and a `.svn` directory, and the project's own repository
on `develop` (the branch Composer guesses for `delta` and `zeta`). Its
`composer.lock` was captured from Composer 2.10.3 on 2026-09-16 on that
tree; the nested commits are reproducible (author, dates and configuration
pinned), so the HEAD references in the lock are stable.
