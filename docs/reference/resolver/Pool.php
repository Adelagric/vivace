<?php declare(strict_types=1);











namespace Composer\DependencyResolver;

use Composer\Advisory\PartialSecurityAdvisory;
use Composer\Advisory\SecurityAdvisory;
use Composer\FilterList\FilterListEntry;
use Composer\Package\BasePackage;
use Composer\Package\Version\VersionParser;
use Composer\Semver\CompilingMatcher;
use Composer\Semver\Constraint\ConstraintInterface;
use Composer\Semver\Constraint\Constraint;







class Pool implements \Countable
{

protected $packages = [];

protected $packageByName = [];

protected $versionParser;

protected $providerCache = [];

protected $unacceptableFixedOrLockedPackages;

protected $removedVersions = [];

protected $removedVersionsByPackage = [];

private $securityRemovedVersions = [];

private $abandonedRemovedVersions = [];

private $filterListRemovedVersions = [];










public function __construct(array $packages = [], array $unacceptableFixedOrLockedPackages = [], array $removedVersions = [], array $removedVersionsByPackage = [], array $securityRemovedVersions = [], array $abandonedRemovedVersions = [], array $filterListRemovedVersions = [])
{
$this->versionParser = new VersionParser;
$this->setPackages($packages);
$this->unacceptableFixedOrLockedPackages = $unacceptableFixedOrLockedPackages;
$this->removedVersions = $removedVersions;
$this->removedVersionsByPackage = $removedVersionsByPackage;
$this->securityRemovedVersions = $securityRemovedVersions;
$this->abandonedRemovedVersions = $abandonedRemovedVersions;
$this->filterListRemovedVersions = $filterListRemovedVersions;
}




public function getRemovedVersions(string $name, ConstraintInterface $constraint): array
{
if (!isset($this->removedVersions[$name])) {
return [];
}

$result = [];
foreach ($this->removedVersions[$name] as $version => $prettyVersion) {
if ($constraint->matches(new Constraint('==', $version))) {
$result[$version] = $prettyVersion;
}
}

return $result;
}




public function getAllRemovedVersions(): array
{
return $this->removedVersions;
}




public function getRemovedVersionsByPackage(int $objectId): array
{
if (!isset($this->removedVersionsByPackage[$objectId])) {
return [];
}

return $this->removedVersionsByPackage[$objectId];
}




public function getAllRemovedVersionsByPackage(): array
{
return $this->removedVersionsByPackage;
}

public function isSecurityRemovedPackageVersion(string $packageName, ?ConstraintInterface $constraint): bool
{
foreach ($this->securityRemovedVersions[$packageName] ?? [] as $version => $packageWithSecurityAdvisories) {
if ($constraint !== null && $constraint->matches(new Constraint('==', $version))) {
return true;
}
}

return false;
}




public function getSecurityAdvisoryIdentifiersForPackageVersion(string $packageName, ?ConstraintInterface $constraint): array
{
foreach ($this->securityRemovedVersions[$packageName] ?? [] as $version => $packageWithSecurityAdvisories) {
if ($constraint !== null && $constraint->matches(new Constraint('==', $version))) {
return array_map(static function ($advisory) {
return $advisory->advisoryId;
}, $packageWithSecurityAdvisories);
}
}

return [];
}

public function isAbandonedRemovedPackageVersion(string $packageName, ?ConstraintInterface $constraint): bool
{
foreach ($this->abandonedRemovedVersions[$packageName] ?? [] as $version => $prettyVersion) {
if ($constraint !== null && $constraint->matches(new Constraint('==', $version))) {
return true;
}
}

return false;
}




public function getAllSecurityRemovedPackageVersions(): array
{
return $this->securityRemovedVersions;
}




public function getAllAbandonedRemovedPackageVersions(): array
{
return $this->abandonedRemovedVersions;
}

public function isFilterListRemovedPackageVersion(string $packageName, ?ConstraintInterface $constraint): bool
{
foreach ($this->filterListRemovedVersions[$packageName] ?? [] as $version => $entries) {
if ($constraint !== null && $constraint->matches(new Constraint('==', $version))) {
return true;
}
}

return false;
}




public function getAllFilterListRemovedPackageVersions(): array
{
return $this->filterListRemovedVersions;
}




public function getFilterListEntryForPackageVersion(string $packageName, ?ConstraintInterface $constraint): array
{
$lists = [];
$seen = [];
foreach ($this->filterListRemovedVersions[$packageName] ?? [] as $version => $filterListEntries) {
if ($constraint !== null && $constraint->matches(new Constraint('==', $version))) {
foreach ($filterListEntries as $entry) {
$entryKey = spl_object_id($entry);
if (isset($seen[$entryKey])) {
continue;
}

$seen[$entryKey] = true;

$source = (bool) $entry->source ? ' reported by ' . $entry->source : '';
$url = (bool) $entry->url ? ' (see ' . $entry->url . ')' : '';
$reason = (bool) $entry->reason ? ' reason: ' . $entry->reason : '';

$lists[$entry->listName][] = $source . $url . $reason;
}

}
}

$result = [];
foreach ($lists as $listName => $listEntries) {
$action = $listName === 'malware' ? 'flagged as ' : 'filtered by ';
$result[$listName] = $action . $listName . implode(', ', $listEntries);
}

return $result;
}




private function setPackages(array $packages): void
{
$id = 1;

foreach ($packages as $package) {
$this->packages[] = $package;

$package->id = $id++;

foreach ($package->getNames() as $provided) {
$this->packageByName[$provided][] = $package;
}
}
}




public function getPackages(): array
{
return $this->packages;
}




public function packageById(int $id): BasePackage
{
return $this->packages[$id - 1];
}




public function count(): int
{
return \count($this->packages);
}









public function whatProvides(string $name, ?ConstraintInterface $constraint = null): array
{
$key = (string) $constraint;
if (isset($this->providerCache[$name][$key])) {
return $this->providerCache[$name][$key];
}

return $this->providerCache[$name][$key] = $this->computeWhatProvides($name, $constraint);
}







private function computeWhatProvides(string $name, ?ConstraintInterface $constraint = null): array
{
if (!isset($this->packageByName[$name])) {
return [];
}

$matches = [];

foreach ($this->packageByName[$name] as $candidate) {
if ($this->match($candidate, $name, $constraint)) {
$matches[] = $candidate;
}
}

return $matches;
}

public function literalToPackage(int $literal): BasePackage
{
$packageId = abs($literal);

return $this->packageById($packageId);
}




public function literalToPrettyString(int $literal, array $installedMap): string
{
$package = $this->literalToPackage($literal);

if (isset($installedMap[$package->id])) {
$prefix = ($literal > 0 ? 'keep' : 'remove');
} else {
$prefix = ($literal > 0 ? 'install' : 'don\'t install');
}

return $prefix.' '.$package->getPrettyString();
}







public function match(BasePackage $candidate, string $name, ?ConstraintInterface $constraint = null): bool
{
$candidateName = $candidate->getName();
$candidateVersion = $candidate->getVersion();

if ($candidateName === $name) {
return $constraint === null || CompilingMatcher::match($constraint, Constraint::OP_EQ, $candidateVersion);
}

$provides = $candidate->getProvides();
$replaces = $candidate->getReplaces();


if (isset($replaces[0]) || isset($provides[0])) {
foreach ($provides as $link) {
if ($link->getTarget() === $name && ($constraint === null || $constraint->matches($link->getConstraint()))) {
return true;
}
}

foreach ($replaces as $link) {
if ($link->getTarget() === $name && ($constraint === null || $constraint->matches($link->getConstraint()))) {
return true;
}
}

return false;
}

if (isset($provides[$name]) && ($constraint === null || $constraint->matches($provides[$name]->getConstraint()))) {
return true;
}

if (isset($replaces[$name]) && ($constraint === null || $constraint->matches($replaces[$name]->getConstraint()))) {
return true;
}

return false;
}

public function isUnacceptableFixedOrLockedPackage(BasePackage $package): bool
{
return \in_array($package, $this->unacceptableFixedOrLockedPackages, true);
}




public function getUnacceptableFixedOrLockedPackages(): array
{
return $this->unacceptableFixedOrLockedPackages;
}

public function __toString(): string
{
$str = "Pool:\n";

foreach ($this->packages as $package) {
$str .= '- '.str_pad((string) $package->id, 6, ' ', STR_PAD_LEFT).': '.$package->getName()."\n";
}

return $str;
}
}
