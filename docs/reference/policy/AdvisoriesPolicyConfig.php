<?php declare(strict_types=1);











namespace Composer\Policy;

use Composer\Semver\VersionParser;






class AdvisoriesPolicyConfig extends ListPolicyConfig
{
public const NAME = 'advisories';




public $ignoreId;




public $ignoreSeverity;







public function __construct(
bool $block,
string $audit,
array $ignore,
array $ignoreId,
array $ignoreSeverity
) {
parent::__construct(
self::NAME,
$block,
$audit,
$ignore
);
$this->ignoreId = $ignoreId;
$this->ignoreSeverity = $ignoreSeverity;
}







public function getIgnoreIdForOperation(string $operation): array
{
$result = [];
foreach ($this->ignoreId as $id => $rule) {
if (($operation === 'block' && $rule->onBlock) || ($operation === 'audit' && $rule->onAudit)) {
$result[$id] = $rule->reason;
}
}

return $result;
}












public function getIgnoreListForOperation(string $operation): array
{
$result = $this->getIgnoreIdForOperation($operation);

foreach ($this->getFlatIgnoreForOperation($operation) as $packageName => $reason) {
$result[$packageName] = static::mergeReason($result, $packageName, $reason);
}

return $result;
}







public function getIgnoreSeverityForOperation(string $operation): array
{
$result = [];
foreach ($this->ignoreSeverity as $severity => $rule) {
if (($operation === 'block' && $rule->onBlock) || ($operation === 'audit' && $rule->onAudit)) {
$result[$severity] = $rule->reason;
}
}

return $result;
}

public function withBlockingDisabled()
{
return new static(
false,
$this->audit,
$this->ignore,
$this->ignoreId,
$this->ignoreSeverity
);
}

public function withAudit(string $audit)
{
return new static(
$this->block,
$audit,
$this->ignore,
$this->ignoreId,
$this->ignoreSeverity
);
}









public function withIgnoreSeverity(array $severities)
{
$ignoreSeverity = $this->ignoreSeverity;
foreach ($severities as $severity) {
if (!isset($ignoreSeverity[$severity])) {
$ignoreSeverity[$severity] = new IgnoreSeverityRule($severity, null, false, true);
}
}

return new static(
$this->block,
$this->audit,
$this->ignore,
$this->ignoreId,
$ignoreSeverity
);
}





public static function fromRawConfig(array $policyConfig, array $auditConfig, VersionParser $parser): self
{
if (!isset($policyConfig['advisories']) && $auditConfig !== []) {
$legacyIgnore = parent::parseLegacyAuditIgnore($auditConfig['ignore'] ?? [], $parser);

return new self(
$auditConfig['block-insecure'] ?? true,
self::AUDIT_FAIL,
$legacyIgnore['packages'] ?? [],
$legacyIgnore['ids'] ?? [],
self::parseLegacySeverityWithApply($auditConfig['ignore-severity'] ?? [])
);
}

$advisoryConfig = $policyConfig['advisories'] ?? [];
if ($advisoryConfig === false) {
return self::disabled();
}

if (!is_array($advisoryConfig)) {
$advisoryConfig = [];
}

return new self(
(bool) ($advisoryConfig['block'] ?? true),
$advisoryConfig['audit'] ?? self::AUDIT_FAIL,
IgnorePackageRule::parseIgnoreMap($advisoryConfig['ignore'] ?? [], $parser),
IgnoreIdRule::parseIgnoreIdMap($advisoryConfig['ignore-id'] ?? []),
IgnoreSeverityRule::parseIgnoreSeverityMap($advisoryConfig['ignore-severity'] ?? [])
);
}

public static function disabled(): self
{
return new self(
false,
self::AUDIT_IGNORE,
[],
[],
[]
);
}





private static function parseLegacySeverityWithApply(array $config): array
{
$result = [];
foreach ($config as $key => $value) {
$severity = is_int($key) ? (string) $value : $key;
$parsed = self::parseLegacySingleIgnore($key, $value);
$result[$severity] = new IgnoreSeverityRule($severity, $parsed['reason'], $parsed['onBlock'], $parsed['onAudit']);
}

return $result;
}
}
