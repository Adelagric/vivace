<?php declare(strict_types=1);











namespace Composer\FilterList;

use Composer\Policy\PolicyConfig;






class ComposerRepositoryFilterInformation
{



public $metadata;




public $lists;




public $summaryUrl;




public $apiUrl;




private function __construct(bool $metadata, array $lists, ?string $summaryUrl, ?string $apiUrl)
{
$this->metadata = $metadata;
$this->lists = $lists;
$this->summaryUrl = $summaryUrl;
$this->apiUrl = $apiUrl;
}





public static function fromData(array $data, ?\Closure $canonicalizeUrl = null): self
{
$lists = [];
if (isset($data['lists']) && is_array($data['lists'])) {
foreach ($data['lists'] as $name => $config) {
if (!is_string($name) || !is_array($config) || !(bool) ($config['enabled'] ?? false)) {
continue;
}
$lists[] = $name;
}
}




$lists = array_values(array_filter($lists, static function (string $name): bool {
if (in_array($name, PolicyConfig::RESERVED_NAMES, true) || in_array($name, PolicyConfig::FUTURE_RESERVED_NAMES, true)) {
return false;
}

foreach (PolicyConfig::FUTURE_RESERVED_PREFIXES as $prefix) {
if (str_starts_with($name, $prefix)) {
return false;
}
}

return true;
}));

$summaryUrl = null;
if (isset($data['summary-url']) && is_string($data['summary-url']) && $data['summary-url'] !== '') {
$summaryUrl = $canonicalizeUrl !== null ? $canonicalizeUrl($data['summary-url']) : $data['summary-url'];
}

$apiUrl = null;
if (isset($data['api-url']) && is_string($data['api-url']) && $data['api-url'] !== '') {
$apiUrl = $canonicalizeUrl !== null ? $canonicalizeUrl($data['api-url']) : $data['api-url'];
}

return new self(
(bool) ($data['metadata'] ?? false),
$lists,
$summaryUrl,
$apiUrl
);
}
}
