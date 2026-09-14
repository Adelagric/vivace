<?php declare(strict_types=1);











namespace Composer\Policy;

use Composer\FilterList\Source\SourceValidator;
use Composer\FilterList\Source\UrlSource;
use Composer\Semver\VersionParser;






class CustomListPolicyConfig extends ListPolicyConfig
{




public $sources;






public function __construct(
string $name,
bool $block,
string $audit,
array $ignore,
array $sources
) {
parent::__construct(
$name,
$block,
$audit,
$ignore
);

$this->sources = $sources;
}

public function withBlockingDisabled()
{
return new static(
$this->name,
false,
$this->audit,
$this->ignore,
$this->sources
);
}

public function withAudit(string $audit)
{
return new static(
$this->name,
$this->block,
$audit,
$this->ignore,
$this->sources
);
}




public static function fromRawConfig(string $listName, $listConfig, VersionParser $parser): self
{
if ($listConfig === false) {
return self::disabled($listName);
}

if ($listConfig === true) {
$listConfig = [];
}

if (!is_array($listConfig)) {
return self::disabled($listName);
}

$sources = [];
$sourceValidator = new SourceValidator();
foreach ($listConfig['sources'] ?? [] as $sourceConfig) {
if (is_array($sourceConfig)) {
$sources[] = $sourceValidator->validate($listName, $sourceConfig);
}
}

return new self(
$listName,
(bool) ($listConfig['block'] ?? true),
$listConfig['audit'] ?? self::AUDIT_FAIL,
IgnorePackageRule::parseIgnoreMap($listConfig['ignore'] ?? [], $parser),
$sources
);
}

public static function disabled(string $listName): self
{
return new static(
$listName,
false,
self::AUDIT_IGNORE,
[],
[]
);
}
}
