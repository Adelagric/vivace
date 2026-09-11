<?php declare(strict_types=1);











namespace Composer\DependencyResolver;

use Composer\Package\AliasPackage;
use Composer\Package\BasePackage;
use Composer\Package\Version\VersionParser;
use Composer\Semver\CompilingMatcher;
use Composer\Semver\Constraint\ConstraintInterface;
use Composer\Semver\Constraint\Constraint;
use Composer\Semver\Constraint\MultiConstraint;
use Composer\Semver\Intervals;






class PoolOptimizer
{



private $policy;




private $irremovablePackages = [];




private $requireConstraintsPerPackage = [];




private $conflictConstraintsPerPackage = [];




private $packagesToRemove = [];




private $aliasesPerPackage = [];




private $removedVersionsByPackage = [];

public function __construct(PolicyInterface $policy)
{
$this->policy = $policy;
}

public function optimize(Request $request, Pool $pool): Pool
{
$this->prepare($request, $pool);

$this->optimizeByIdenticalDependencies($request, $pool);

$this->optimizeImpossiblePackagesAway($request, $pool);

$optimizedPool = $this->applyRemovalsToPool($pool);






$this->irremovablePackages = [];
$this->requireConstraintsPerPackage = [];
$this->conflictConstraintsPerPackage = [];
$this->packagesToRemove = [];
$this->aliasesPerPackage = [];
$this->removedVersionsByPackage = [];

return $optimizedPool;
}

private function prepare(Request $request, Pool $pool): void
{
$irremovablePackageConstraintGroups = [];


foreach ($request->getFixedOrLockedPackages() as $package) {
$irremovablePackageConstraintGroups[$package->getName()][] = new Constraint('==', $package->getVersion());
}


foreach ($request->getRequires() as $require => $constraint) {
$this->extractRequireConstraintsPerPackage($require, $constraint);
}


foreach ($pool->getPackages() as $package) {

foreach ($package->getRequires() as $link) {
$this->extractRequireConstraintsPerPackage($link->getTarget(), $link->getConstraint());
}

foreach ($package->getConflicts() as $link) {
$this->extractConflictConstraintsPerPackage($link->getTarget(), $link->getConstraint());
}



if ($package instanceof AliasPackage) {
$this->aliasesPerPackage[$package->getAliasOf()->id][] = $package;
}
}

$irremovablePackageConstraints = [];
foreach ($irremovablePackageConstraintGroups as $packageName => $constraints) {
$irremovablePackageConstraints[$packageName] = 1 === \count($constraints) ? $constraints[0] : new MultiConstraint($constraints, false);
}
unset($irremovablePackageConstraintGroups);


foreach ($pool->getPackages() as $package) {
if (!isset($irremovablePackageConstraints[$package->getName()])) {
continue;
}

if (CompilingMatcher::match($irremovablePackageConstraints[$package->getName()], Constraint::OP_EQ, $package->getVersion())) {
$this->markPackageIrremovable($package);
}
}
}

private function markPackageIrremovable(BasePackage $package): void
{
$this->irremovablePackages[$package->id] = true;
if ($package instanceof AliasPackage) {


$this->markPackageIrremovable($package->getAliasOf());
}
if (isset($this->aliasesPerPackage[$package->id])) {
foreach ($this->aliasesPerPackage[$package->id] as $aliasPackage) {
$this->irremovablePackages[$aliasPackage->id] = true;
}
}
}




private function applyRemovalsToPool(Pool $pool): Pool
{
$packages = [];
$removedVersions = [];
foreach ($pool->getPackages() as $package) {
if (!isset($this->packagesToRemove[$package->id])) {
$packages[] = $package;
} else {
$removedVersions[$package->getName()][$package->getVersion()] = $package->getPrettyVersion();
}
}

$optimizedPool = new Pool($packages, $pool->getUnacceptableFixedOrLockedPackages(), $removedVersions, $this->removedVersionsByPackage, $pool->getAllSecurityRemovedPackageVersions(), $pool->getAllAbandonedRemovedPackageVersions(), $pool->getAllFilterListRemovedPackageVersions());

return $optimizedPool;
}

private function optimizeByIdenticalDependencies(Request $request, Pool $pool): void
{
$identicalDefinitionsPerPackage = [];

foreach ($pool->getPackages() as $package) {



if (isset($this->irremovablePackages[$package->id])) {
continue;
}

$this->markPackageForRemoval($package->id);

$dependencyHash = $this->calculateDependencyHash($package);

foreach ($package->getNames(false) as $packageName) {
if (!isset($this->requireConstraintsPerPackage[$packageName])) {
continue;
}

foreach ($this->requireConstraintsPerPackage[$packageName] as $requireConstraint) {
$groupHashParts = [];

if (CompilingMatcher::match($requireConstraint, Constraint::OP_EQ, $package->getVersion())) {
$groupHashParts[] = 'require:' . (string) $requireConstraint;
}

if (\count($package->getReplaces()) > 0) {
foreach ($package->getReplaces() as $link) {
if (CompilingMatcher::match($link->getConstraint(), Constraint::OP_EQ, $package->getVersion())) {

$groupHashParts[] = 'require:' . (string) $link->getConstraint();
}
}
}

if (isset($this->conflictConstraintsPerPackage[$packageName])) {
foreach ($this->conflictConstraintsPerPackage[$packageName] as $conflictConstraint) {
if (CompilingMatcher::match($conflictConstraint, Constraint::OP_EQ, $package->getVersion())) {
$groupHashParts[] = 'conflict:' . (string) $conflictConstraint;
}
}
}

if (0 === \count($groupHashParts)) {
continue;
}

$groupHash = implode('', $groupHashParts);
$identicalDefinitionsPerPackage[$packageName][$groupHash][$dependencyHash][] = $package->id;
}
}
}

foreach ($identicalDefinitionsPerPackage as $packageName => $constraintGroups) {
foreach ($constraintGroups as $constraintGroup) {
foreach ($constraintGroup as $packageIds) {

if (1 === \count($packageIds)) {
$this->keepPackageInGroup($pool->packageById($packageIds[0]), $pool, $packageName, $packageIds);
continue;
}



foreach ($this->policy->selectPreferredPackages($pool, $packageIds) as $preferredLiteral) {
$this->keepPackageInGroup($pool->literalToPackage($preferredLiteral), $pool, $packageName, $packageIds);
}
}
}
}
}

private function calculateDependencyHash(BasePackage $package): string
{
$hash = '';

$hashRelevantLinks = [
'requires' => $package->getRequires(),
'conflicts' => $package->getConflicts(),
'replaces' => $package->getReplaces(),
'provides' => $package->getProvides(),
];

foreach ($hashRelevantLinks as $key => $links) {
if (0 === \count($links)) {
continue;
}


$hash .= $key . ':';

$subhash = [];

foreach ($links as $link) {




$subhash[$link->getTarget()] = (string) $link->getConstraint();
}


ksort($subhash);

foreach ($subhash as $target => $constraint) {
$hash .= $target . '@' . $constraint;
}
}

return $hash;
}

private function markPackageForRemoval(int $id): void
{

if (isset($this->irremovablePackages[$id])) {
throw new \LogicException('Attempted removing a package which was previously marked irremovable');
}

$this->packagesToRemove[$id] = true;
}




private function keepPackageInGroup(BasePackage $package, Pool $pool, string $packageName, array $packageIds): void
{
$versions = [];
foreach ($packageIds as $packageId) {
$groupPackage = $pool->packageById($packageId);
if ($groupPackage instanceof AliasPackage && $groupPackage->getPrettyVersion() === VersionParser::DEFAULT_BRANCH_ALIAS) {
$groupPackage = $groupPackage->getAliasOf();
}
$versions[$groupPackage->getVersion()] = $groupPackage->getPrettyVersion();
}



$this->recordRemovedVersionsForPackage($package, $packageName, $versions);


if (!isset($this->packagesToRemove[$package->id])) {
return;
}

$this->unmarkPackageForRemoval($package);

if ($package instanceof AliasPackage) {
$this->unmarkPackageForRemoval($package->getAliasOf());
$this->recordRemovedVersionsForPackage($package->getAliasOf(), $packageName, $versions);

if (isset($this->aliasesPerPackage[$package->getAliasOf()->id])) {
foreach ($this->aliasesPerPackage[$package->getAliasOf()->id] as $aliasPackage) {
$this->unmarkPackageForRemoval($aliasPackage);
$this->recordRemovedVersionsForPackage($aliasPackage, $packageName, $versions);
}
}

return;
}

if (isset($this->aliasesPerPackage[$package->id])) {
foreach ($this->aliasesPerPackage[$package->id] as $aliasPackage) {
$this->unmarkPackageForRemoval($aliasPackage);
$this->recordRemovedVersionsForPackage($aliasPackage, $packageName, $versions);
}
}
}

private function unmarkPackageForRemoval(BasePackage $package): void
{
unset($this->packagesToRemove[$package->id]);
}




private function recordRemovedVersionsForPackage(BasePackage $package, string $packageName, array $versions): void
{
if (!in_array($packageName, $package->getNames(false), true)) {
return;
}

foreach ($versions as $version => $prettyVersion) {
$this->removedVersionsByPackage[spl_object_id($package)][$version] = $prettyVersion;
}
}






private function optimizeImpossiblePackagesAway(Request $request, Pool $pool): void
{
if (\count($request->getLockedPackages()) === 0) {
return;
}

$packageIndex = [];

foreach ($pool->getPackages() as $package) {
$id = $package->id;


if (isset($this->irremovablePackages[$id])) {
continue;
}

if (isset($this->aliasesPerPackage[$id]) || $package instanceof AliasPackage) {
continue;
}

if ($request->isFixedPackage($package) || $request->isLockedPackage($package)) {
continue;
}

$packageIndex[$package->getName()][$package->id] = $package;
}

foreach ($request->getLockedPackages() as $package) {


$isUnusedPackage = true;
foreach ($package->getNames(false) as $packageName) {
if (isset($this->requireConstraintsPerPackage[$packageName])) {
$isUnusedPackage = false;
break;
}
}

if ($isUnusedPackage) {
continue;
}

foreach ($package->getRequires() as $link) {
$require = $link->getTarget();
if (!isset($packageIndex[$require])) {
continue;
}

$linkConstraint = $link->getConstraint();
foreach ($packageIndex[$require] as $id => $requiredPkg) {
if (false === CompilingMatcher::match($linkConstraint, Constraint::OP_EQ, $requiredPkg->getVersion())) {
$this->markPackageForRemoval($id);
unset($packageIndex[$require][$id]);
}
}
}
}
}








private function extractRequireConstraintsPerPackage(string $package, ConstraintInterface $constraint)
{
foreach ($this->expandDisjunctiveMultiConstraints($constraint) as $expanded) {
$this->requireConstraintsPerPackage[$package][(string) $expanded] = $expanded;
}
}








private function extractConflictConstraintsPerPackage(string $package, ConstraintInterface $constraint)
{
foreach ($this->expandDisjunctiveMultiConstraints($constraint) as $expanded) {
$this->conflictConstraintsPerPackage[$package][(string) $expanded] = $expanded;
}
}




private function expandDisjunctiveMultiConstraints(ConstraintInterface $constraint)
{
$constraint = Intervals::compactConstraint($constraint);

if ($constraint instanceof MultiConstraint && $constraint->isDisjunctive()) {


return $constraint->getConstraints();
}


return [$constraint];
}
}
