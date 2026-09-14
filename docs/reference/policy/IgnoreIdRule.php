<?php declare(strict_types=1);











namespace Composer\Policy;








class IgnoreIdRule
{

public $id;


public $reason;


public $onBlock;


public $onAudit;

public function __construct(string $id, ?string $reason = null, bool $onBlock = true, bool $onAudit = true)
{
$this->id = $id;
$this->reason = $reason;
$this->onBlock = $onBlock;
$this->onAudit = $onAudit;
}












public static function parseIgnoreIdMap(array $config): array
{
$rules = [];

foreach ($config as $key => $value) {
if (is_int($key)) {
if (!is_string($value)) {
throw new \UnexpectedValueException(sprintf(
'Invalid ignore-id entry at index %d: expected an advisory ID string, got %s.',
$key,
get_debug_type($value)
));
}
$rules[$value] = new self($value);
continue;
}

if ($value === null) {
$rules[$key] = new self($key);
continue;
}

if (is_string($value)) {
$rules[$key] = new self($key, $value);
continue;
}

if (is_array($value)) {
$rules[$key] = new self(
$key,
$value['reason'] ?? null,
$value['on-block'] ?? true,
$value['on-audit'] ?? true
);
continue;
}

throw new \UnexpectedValueException(sprintf(
'Invalid ignore-id entry for "%s": value of type %s is not a supported shape.'
. ' Expected null, a reason string, or a rule object.',
$key,
get_debug_type($value)
));
}

return $rules;
}
}
