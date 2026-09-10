# Fixture projects

Frozen application skeletons used by the differential harness. They are
copied verbatim (vendor/, node_modules/, var/ and .env excluded) from the
upstream `create-project` templates, together with the `composer.lock` that
was resolved on 2026-09-09/10; freezing them keeps the harness deterministic
and independent of upstream churn. They are test data, not part of vivace.

| fixture | upstream | license |
|---|---|---|
| laravel | laravel/laravel | MIT |
| symfony | symfony/demo (symfony/symfony-demo) | MIT (see its LICENSE) |
| sylius | sylius/sylius-standard | MIT |
| rector | rectorphp/rector-src | MIT |
| wordpress | (own manifest, see below) | MIT for the manifest; packages have their own licenses |

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
