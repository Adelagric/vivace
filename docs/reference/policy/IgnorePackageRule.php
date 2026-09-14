<?php declare(strict_types=1);











namespace Composer\Policy;

use Composer\Package\BasePackage;
use Composer\Semver\Constraint\ConstraintInterface;
use Composer\Semver\Constraint\MatchAllConstraint;
use Composer\Semver\VersionParser;













class IgnorePackageRule
{

public $packageName;


public $constraint;


public $reason;


public $onBlock;


public $onAudit;


public $packageNameRegex;

public function __construct(
string $packageName,
ConstraintInterface $constraint,
?string $reason = null,
bool $onBlock = true,
bool $onAudit = true
) {
$this->packageName = $packageName;
$this->constraint = $constraint;
$this->reason = $reason;
$this->onBlock = $onBlock;
$this->onAudit = $onAudit;
$this->packageNameRegex = BasePackage::packageNameToRegexp($packageName);
}













public static function parseIgnoreMap(array $config, ?VersionParser $parser = null): array
{
if ($parser === null) {
$parser = new VersionParser();
}

$rules = [];

foreach ($config as $key => $value) {

if ($value === null) {
$rules[$key][] = new self((string) $key, new MatchAllConstraint());
continue;
}


if (is_string($value) && !is_int($key)) {
$rules[$key][] = new self($key, new MatchAllConstraint(), $value);
continue;
}


if (is_int($key) && is_string($value)) {
$rules[$value][] = new self($value, new MatchAllConstraint());
continue;
}


if (is_array($value) && !is_int($key) && isset($value[0])) {
foreach ($value as $ruleConfig) {
if (!is_array($ruleConfig)) {
throw new \UnexpectedValueException(sprintf(
'Invalid ignore rule for "%s": expected an object, got %s.',
$key,
get_debug_type($ruleConfig)
));
}
$rules[$key][] = self::fromRuleObject($key, $ruleConfig, $parser);
}
continue;
}


if (is_array($value) && !is_int($key)) {
$rules[$key][] = self::fromRuleObject($key, $value, $parser);
continue;
}

throw new \UnexpectedValueException(sprintf(
'Invalid ignore entry at key "%s": value of type %s is not a supported shape.'
. ' Expected null, a reason string, a rule object, or a list of rule objects.',
$key,
get_debug_type($value)
));
}

return $rules;
}




private static function fromRuleObject(string $packageName, array $config, VersionParser $parser): self
{
$constraint = isset($config['constraint'])
? $parser->parseConstraints((string) $config['constraint'])
: new MatchAllConstraint();

return new self(
$packageName,
$constraint,
$config['reason'] ?? null,
$config['on-block'] ?? true,
$config['on-audit'] ?? true
);
}








public static function filterByOperation(array $rules, string $operation): array
{
$filtered = [];
foreach ($rules as $packageName => $ruleList) {
foreach ($ruleList as $rule) {
if (($operation === 'block' && $rule->onBlock) || ($operation === 'audit' && $rule->onAudit)) {
$filtered[$packageName][] = $rule;
}
}
}

return $filtered;
}
}
