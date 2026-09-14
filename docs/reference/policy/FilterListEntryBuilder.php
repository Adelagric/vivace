<?php declare(strict_types=1);











namespace Composer\FilterList;

use Composer\Package\Version\VersionParser;
use Composer\Semver\Constraint\ConstraintInterface;








class FilterListEntryBuilder
{

private $versionParser;

public function __construct(?VersionParser $versionParser = null)
{
$this->versionParser = $versionParser ?? new VersionParser();
}









public function build(array $rawByList, array $packageConstraintMap, ?string $defaultPackage = null): array
{
$result = [];
foreach ($rawByList as $listName => $entries) {
if (!is_string($listName) || !is_array($entries)) {
continue;
}
foreach ($entries as $data) {
if (!is_array($data)) {
continue;
}

if (!isset($data['constraint'])) {
continue;
}

if (!isset($data['package'])) {
if ($defaultPackage === null) {
continue;
}

$data['package'] = $defaultPackage;
}

$entry = FilterListEntry::create($listName, $data, $this->versionParser);

if (!isset($packageConstraintMap[$entry->packageName])) {
continue;
}

if (!$entry->constraint->matches($packageConstraintMap[$entry->packageName])) {
continue;
}

$result[$listName][] = $entry;
}
}

return $result;
}
}
