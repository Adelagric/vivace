<?php declare(strict_types=1);











namespace Composer\Policy;






class IgnoreUnreachable
{
public const SCOPES = ['audit', 'install', 'update'];


public $audit;


public $install;


public $update;

public function __construct(bool $audit, bool $install, bool $update)
{
$this->audit = $audit;
$this->install = $install;
$this->update = $update;
}




public function forBlockScope(string $blockScope): bool
{
return $blockScope === ListPolicyConfig::BLOCK_SCOPE_INSTALL
? $this->install
: $this->update;
}

public static function default(): self
{
return new self(false, true, true);
}

public static function all(): self
{
return new self(true, true, true);
}

public static function none(): self
{
return new self(false, false, false);
}






public function with(string ...$scopes): self
{
if (count($scopes) === 0) {
throw new \InvalidArgumentException('At least one scope is required.');
}

$audit = $this->audit;
$install = $this->install;
$update = $this->update;
foreach ($scopes as $scope) {
if (!in_array($scope, self::SCOPES, true)) {
throw new \InvalidArgumentException(sprintf('Unknown scope "%s". Expected one of %s.', $scope, implode(', ', self::SCOPES)));
}
if ($scope === 'audit') {
$audit = true;
} elseif ($scope === 'install') {
$install = true;
} elseif ($scope === 'update') {
$update = true;
}
}

return new self($audit, $install, $update);
}




public static function fromRawPolicyConfig(array $config): self
{
if (!isset($config['ignore-unreachable'])) {
return self::default();
}

if (is_array($config['ignore-unreachable'])) {
return new self(
in_array('audit', $config['ignore-unreachable'], true),
in_array('install', $config['ignore-unreachable'], true),
in_array('update', $config['ignore-unreachable'], true)
);
}

return $config['ignore-unreachable'] ? self::all() : self::none();
}




public static function fromRawAuditConfig(array $auditConfig): self
{
if (isset($auditConfig['ignore-unreachable']) && $auditConfig['ignore-unreachable']) {
return new self(true, false, false);
}

return self::default();
}
}
