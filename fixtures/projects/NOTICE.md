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

`rector` is a library, not an application template, and upstream does not
commit a `composer.lock`: the skeleton holds `composer.json`, the lock
resolved on 2026-09-10 with `composer update --no-install`, and only the
autoload entry points (the `files` entries verbatim, the psr-4/classmap
directories as empty placeholders). It covers `dev-main` packages flagged
`default-branch`, `phpstan/extension-installer` + `rector/extension-installer`
and `platform-check: false`. Its boot check is `vendor/bin/phpstan --version`.
