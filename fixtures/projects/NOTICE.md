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
