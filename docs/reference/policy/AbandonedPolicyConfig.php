<?php declare(strict_types=1);











namespace Composer\Policy;

use Composer\Semver\VersionParser;






class AbandonedPolicyConfig extends ListPolicyConfig
{
public const NAME = 'abandoned';

public function __construct(
bool $block,
string $audit,
array $ignore
) {
parent::__construct(
self::NAME,
$block,
$audit,
$ignore
);
}

public function withBlockingDisabled()
{
return new static(
false,
$this->audit,
$this->ignore
);
}

public function withAudit(string $audit)
{
return new static(
$this->block,
$audit,
$this->ignore
);
}





public static function fromRawConfig(array $policyConfig, array $auditConfig, VersionParser $parser): self
{
if (!isset($policyConfig['abandoned']) && $auditConfig !== []) {
return new self(
$auditConfig['block-abandoned'] ?? false,
$auditConfig['abandoned'] ?? ListPolicyConfig::AUDIT_FAIL,
self::parseLegacyIgnoreWithApply($auditConfig['ignore-abandoned'] ?? [])
);
}

$abandonedConfig = $policyConfig['abandoned'] ?? [];
if ($abandonedConfig === false) {
return self::disabled();
}

if (!is_array($abandonedConfig)) {
$abandonedConfig = [];
}

return new self(
(bool) ($abandonedConfig['block'] ?? false),
$abandonedConfig['audit'] ?? self::AUDIT_FAIL,
IgnorePackageRule::parseIgnoreMap($abandonedConfig['ignore'] ?? [], $parser)
);
}

public static function disabled(): self
{
return new self(
false,
self::AUDIT_IGNORE,
[]
);
}
}
