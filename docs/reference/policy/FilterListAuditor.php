<?php declare(strict_types=1);











namespace Composer\FilterList;

use Composer\FilterList\FilterListProvider\FilterListProviderSet;
use Composer\Package\BasePackage;
use Composer\Package\PackageInterface;
use Composer\Package\RootPackageInterface;
use Composer\Pcre\Preg;
use Composer\Policy\ListPolicyConfig;
use Composer\Policy\MalwarePolicyConfig;
use Composer\Policy\PolicyConfig;
use Composer\Semver\Constraint\Constraint;
use Composer\Semver\Constraint\ConstraintInterface;






class FilterListAuditor
{





public function collectFilterLists(array $packages, FilterListProviderSet $providerSet, array $configuredLists, bool $ignoreUnreachable): array
{
$result = $providerSet->getMatchingFilterLists($packages, $configuredLists, $ignoreUnreachable);
$filter = $result['filter'];
$unreachableRepos = $result['unreachableRepos'];

$filterListMap = [];
foreach ($filter as $entries) {
foreach ($entries as $entry) {
$filterListMap[$entry->packageName][$entry->listName][] = $entry;
}
}

ksort($filterListMap);

return ['filter' => $filterListMap, 'unreachableRepos' => $unreachableRepos];
}





public function getMatchingAuditEntries(PackageInterface $package, array $filterListMap, PolicyConfig $policyConfig): array
{
return $this->matchingEntries($package, $filterListMap, $policyConfig->getActiveAuditFilterLists(), 'audit');
}






public function getMatchingBlockEntries(PackageInterface $package, array $filterListMap, PolicyConfig $policyConfig, string $blockScope): array
{
return $this->matchingEntries($package, $filterListMap, $policyConfig->getActiveBlockFilterLists($blockScope), 'block');
}







private function matchingEntries(PackageInterface $package, array $filterListMap, array $activeListConfigs, string $operation): array
{
if ($package instanceof RootPackageInterface || count($filterListMap) === 0) {
return [];
}

if (isset($activeListConfigs[MalwarePolicyConfig::NAME]) && $activeListConfigs[MalwarePolicyConfig::NAME] instanceof MalwarePolicyConfig) {
$filterListMap = $this->applyMalwareIgnoreSource($filterListMap, $activeListConfigs[MalwarePolicyConfig::NAME], $operation);
}

$matchingEntries = [];
$allPackageNames = [];
foreach ($activeListConfigs as $activeListConfig) {
$allPackageNames = array_merge($allPackageNames, array_keys($activeListConfig->getIgnoreForOperation($operation)));
}
$allIgnoredPackageNamesRegex = BasePackage::packageNamesToRegexp($allPackageNames);
$packageConstraint = new Constraint(Constraint::STR_OP_EQ, $package->getVersion());

foreach ($package->getNames(false) as $packageName) {
if (!isset($filterListMap[$packageName])) {
continue;
}

$packageEntries = array_intersect_key($filterListMap[$packageName], $activeListConfigs);
if (Preg::isMatch($allIgnoredPackageNamesRegex, $packageName)) {
$unfilteredEntries = [];
foreach ($packageEntries as $listName => $entries) {
if ($this->isPackageIgnored($packageName, $packageConstraint, $activeListConfigs[$listName], $operation)) {
continue;
}

$unfilteredEntries[$listName] = $entries;
}

$packageEntries = $unfilteredEntries;
}

foreach ($packageEntries as $entries) {
foreach ($entries as $entry) {
if ($entry->constraint->matches($packageConstraint)) {
$matchingEntries[] = $entry;
}
}
}
}

return $matchingEntries;
}




private function isPackageIgnored(string $packageName, ConstraintInterface $packageConstraint, ListPolicyConfig $listConfig, string $operation): bool
{
foreach ($listConfig->getIgnoreForOperation($operation) as $ignorePackageRules) {
foreach ($ignorePackageRules as $ignorePackageRule) {
if (Preg::isMatch($ignorePackageRule->packageNameRegex, $packageName) && $ignorePackageRule->constraint->matches($packageConstraint)) {
return true;
}
}
}

return false;
}








private function applyMalwareIgnoreSource(array $filterListMap, MalwarePolicyConfig $malwarePolicyConfig, string $operation): array
{
$ignoreSource = $malwarePolicyConfig->ignoreSource;
if (count($ignoreSource) === 0) {
return $filterListMap;
}

foreach ($filterListMap as $packageName => $entries) {
if (!isset($entries[MalwarePolicyConfig::NAME])) {
continue;
}

$packageEntries = [];
foreach ($entries[MalwarePolicyConfig::NAME] as $malwareEntry) {
if (!in_array($malwareEntry->source, $ignoreSource, true)) {
$packageEntries[] = $malwareEntry;
}
}

if (count($packageEntries) > 0) {
$filterListMap[$packageName][MalwarePolicyConfig::NAME] = $packageEntries;
} else {
unset($filterListMap[$packageName][MalwarePolicyConfig::NAME]);
if (count($filterListMap[$packageName]) === 0) {
unset($filterListMap[$packageName]);
}
}
}

return $filterListMap;
}
}
