<?php declare(strict_types=1);











namespace Composer\FilterList\FilterListProvider;

use Composer\Downloader\TransportException;
use Composer\FilterList\FilterListEntry;
use Composer\FilterList\Source\UrlSource;
use Composer\Package\AliasPackage;
use Composer\Package\PackageInterface;
use Composer\Policy\PolicyConfig;
use Composer\Repository\FilterListProviderInterface;
use Composer\Repository\RepositoryInterface;
use Composer\Semver\Constraint\Constraint;
use Composer\Semver\Constraint\ConstraintInterface;
use Composer\Semver\Constraint\MultiConstraint;
use Composer\Util\HttpDownloader;






class FilterListProviderSet
{

private $providers;

private $unreachableRepoExceptions;





public function __construct(array $repositories, array $sources)
{
$providers = $sources;
$unreachableRepoExceptions = [];
foreach ($repositories as $repository) {
try {
if ($repository instanceof FilterListProviderInterface && $repository->hasFilter()) {
$providers[] = $repository;
}
} catch (\Composer\Downloader\TransportException $e) {
$unreachableRepoExceptions[] = $e;
}

}

$this->providers = $providers;
$this->unreachableRepoExceptions = $unreachableRepoExceptions;
}




public static function create(PolicyConfig $config, array $repositories, HttpDownloader $httpDownloader): self
{
$sources = [];
foreach ($config->getCustomListsWithSources() as $listConfig) {
$sources = array_merge($sources, $listConfig->sources);
}

return new FilterListProviderSet(
array_values($repositories),
array_map(
static function (UrlSource $source) use ($httpDownloader) {
return new UrlSourceFilterListProvider($httpDownloader, $source);
},
$sources
)
);
}






public function getMatchingFilterLists(array $packages, array $configuredLists, bool $ignoreUnreachable = false): array
{
$constraintsByName = [];
foreach ($packages as $package) {

if ($package instanceof AliasPackage && $package->isRootPackageAlias()) {
continue;
}


$constraintsByName[$package->getName()][$package->getVersion()] = new Constraint('=', $package->getVersion());
}

$map = [];
foreach ($constraintsByName as $name => $constraints) {
$map[$name] = MultiConstraint::create(array_values($constraints), false);
}

$unreachableRepos = [];
$filters = $this->getFilterListEntriesForConstraints($map, $configuredLists, $ignoreUnreachable, $unreachableRepos);

return ['filter' => $filters, 'unreachableRepos' => $unreachableRepos];
}







private function getFilterListEntriesForConstraints(array $packageConstraintMap, array $configuredLists, bool $ignoreUnreachable = false, array &$unreachableRepos = []): array
{
foreach ($this->unreachableRepoExceptions as $e) {
if (!$ignoreUnreachable) {
throw $e;
}

$unreachableRepos[] = $e->getMessage();
}

$filters = [];
foreach ($this->providers as $provider) {
$providerLists = $provider->getFilterLists();
$relevantLists = array_values(array_intersect($configuredLists, $providerLists));
if ([] === $relevantLists) {
continue;
}

try {
$result = $provider->getFilter($packageConstraintMap, $relevantLists);
$repoFilter = $result['filter'];

foreach ($repoFilter as $listName => $entries) {
if (!in_array($listName, $configuredLists, true) || !in_array($listName, $providerLists, true)) {
continue;
}

foreach ($entries as $entry) {
if (!isset($packageConstraintMap[$entry->packageName])) {
continue;
}

if (!$entry->constraint->matches($packageConstraintMap[$entry->packageName])) {
continue;
}

$filters[$listName][] = $entry;
}
}

} catch (\Composer\Downloader\TransportException $e) {
if (!$ignoreUnreachable) {
throw $e;
}
$unreachableRepos[] = $e->getMessage();
}
}

ksort($filters);

return $filters;
}
}
