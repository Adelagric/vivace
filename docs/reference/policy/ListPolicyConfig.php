<?php declare(strict_types=1);











namespace Composer\Policy;

use Composer\Semver\Constraint\MatchAllConstraint;
use Composer\Semver\VersionParser;












abstract class ListPolicyConfig
{
public const AUDIT_IGNORE = 'ignore';
public const AUDIT_REPORT = 'report';
public const AUDIT_FAIL = 'fail';


public const AUDITS = [
self::AUDIT_IGNORE,
self::AUDIT_REPORT,
self::AUDIT_FAIL,
];






public const BLOCK_SCOPE_UPDATE = 'update';
public const BLOCK_SCOPE_INSTALL = 'install';
public const BLOCK_SCOPE_ALL = 'all';


public $name;







public $block;


public $audit;





public $ignore;





public function __construct(
string $name,
bool $block,
string $audit,
array $ignore
) {
$this->name = $name;
$this->block = $block;
$this->audit = $audit;
$this->ignore = $ignore;
}

protected function supportsInstallBlockScope(): bool
{
return false;
}









public function shouldBlock(string $blockScope): bool
{
if (!$this->block) {
return false;
}

if ($blockScope === self::BLOCK_SCOPE_INSTALL && !$this->supportsInstallBlockScope()) {
return false;
}

return true;
}







public function getIgnoreForOperation(string $operation): array
{
return IgnorePackageRule::filterByOperation($this->ignore, $operation);
}










public function getFlatIgnoreForOperation(string $operation): array
{
$result = [];
foreach ($this->ignore as $packageName => $rules) {
foreach ($rules as $rule) {
if (($operation === 'block' && $rule->onBlock) || ($operation === 'audit' && $rule->onAudit)) {
$result[$packageName] = self::mergeReason($result, $packageName, $rule->reason);
}
}
}

return $result;
}









protected static function mergeReason(array $map, string $key, ?string $newReason): ?string
{
if (!array_key_exists($key, $map)) {
return $newReason;
}

$existing = $map[$key];

if ($newReason === null) {
return $existing;
}
if ($existing === null) {
return $newReason;
}
if ($existing === $newReason) {
return $existing;
}


$parts = array_map('trim', explode(';', $existing));
if (in_array($newReason, $parts, true)) {
return $existing;
}

return $existing.'; '.$newReason;
}




abstract public function withBlockingDisabled();





abstract public function withAudit(string $audit);










protected static function parseLegacyAuditIgnore(array $config, VersionParser $parser): array
{
$packages = [];
$ids = [];

foreach ($config as $key => $value) {

$id = is_int($key) ? (string) $value : $key;


$isPackageName = strpos($id, '/') !== false;


$parsed = self::parseLegacySingleIgnore($key, $value);

if ($isPackageName) {
$packages[$id][] = new IgnorePackageRule(
$id,
new MatchAllConstraint(),
$parsed['reason'],
$parsed['onBlock'],
$parsed['onAudit']
);
} else {
$ids[$id] = new IgnoreIdRule($id, $parsed['reason'], $parsed['onBlock'], $parsed['onAudit']);
}
}

return ['packages' => $packages, 'ids' => $ids];
}







protected static function parseLegacyIgnoreWithApply(array $config): array
{
$result = [];
foreach ($config as $key => $value) {
$packageName = is_int($key) ? (string) $value : $key;
$parsed = self::parseLegacySingleIgnore($key, $value);
$result[$packageName] = [new IgnorePackageRule($packageName, new MatchAllConstraint(), $parsed['reason'], $parsed['onBlock'], $parsed['onAudit'])];
}

return $result;
}








protected static function parseLegacySingleIgnore($key, $value): array
{
$reason = null;
$onBlock = true;
$onAudit = true;

if (is_int($key) && is_string($value)) {

} elseif (is_string($value)) {

$reason = $value;
} elseif (is_array($value)) {

$apply = $value['apply'] ?? 'all';
$reason = $value['reason'] ?? null;
if (!in_array($apply, ['audit', 'block', 'all'], true)) {
throw new \InvalidArgumentException(sprintf(
"Invalid 'apply' value for '%s': %s. Expected 'audit', 'block', or 'all'.",
(string) $key,
is_string($apply) ? $apply : get_debug_type($apply)
));
}
$onBlock = in_array($apply, ['block', 'all'], true);
$onAudit = in_array($apply, ['audit', 'all'], true);
}

return ['reason' => $reason, 'onBlock' => $onBlock, 'onAudit' => $onAudit];
}
}
