<?php
// Oracle R1 : le pool que Composer construit pour `composer update` dans le
// projet courant (composer.json + composer.lock + config globale de
// COMPOSER_HOME), sans optimiseur ni filtres de sécurité — réplique de
// Installer::doUpdate jusqu'à createPool (createRepositorySet, createRequest,
// requirePackagesForUpdate). Sortie JSON sur stdout : les paquets du pool
// dans l'ordre (l'ordre est celui que le solveur verra).
//
// Usage : COMPOSER_HOME=… COMPOSER_ROOT_VERSION=… php tools/oracle-pool.php <composer.phar> [--no-dev]
declare(strict_types=1);

$phar = $argv[1] ?? null;
if ($phar === null || !is_file($phar)) {
    fwrite(STDERR, "usage: oracle-pool.php <composer.phar>\n");
    exit(2);
}
require "phar://$phar/vendor/autoload.php";

use Composer\DependencyResolver\Request;
use Composer\Factory;
use Composer\IO\NullIO;
use Composer\Package\AliasPackage;
use Composer\Package\BasePackage;
use Composer\Package\RootAliasPackage;
use Composer\Package\Version\VersionParser;
use Composer\Repository\PlatformRepository;
use Composer\Repository\RepositorySet;
use Composer\Repository\RootPackageRepository;
use Composer\Semver\Constraint\Constraint;

$io = new NullIO();
$composer = Factory::create($io, null, true);
$package = $composer->getPackage();
$config = $composer->getConfig();

$platformRepo = new PlatformRepository([], $config->get('platform') ?: []);
$aliases = $package->getAliases();
$locker = $composer->getLocker();
$lockedRepository = $locker->isLocked() ? $locker->getLockedRepository(true) : null;

$minimumStability = $package->getMinimumStability();
$stabilityFlags = $package->getStabilityFlags();
$requires = array_merge($package->getRequires(), $package->getDevRequires());
$rootRequires = [];
foreach ($requires as $req => $link) {
    $rootRequires[$req] = $link->getConstraint();
}
$fixedRootPackage = clone $package;
$fixedRootPackage->setRequires([]);
$fixedRootPackage->setDevRequires([]);
$stabilityFlags[$package->getName()] = BasePackage::STABILITIES[VersionParser::parseStability($package->getVersion())];

$repositorySet = new RepositorySet($minimumStability, $stabilityFlags, $aliases, $package->getReferences(), $rootRequires, []);
$repositorySet->addRepository(new RootPackageRepository($fixedRootPackage));
$repositorySet->addRepository($platformRepo);
foreach ($composer->getRepositoryManager()->getRepositories() as $repository) {
    $repositorySet->addRepository($repository);
}
if ($lockedRepository) {
    $repositorySet->addRepository($lockedRepository);
}

$request = new Request($lockedRepository);
$request->fixPackage($fixedRootPackage);
if ($fixedRootPackage instanceof RootAliasPackage) {
    $request->fixPackage($fixedRootPackage->getAliasOf());
}
$provided = $fixedRootPackage->getProvides();
foreach ($platformRepo->getPackages() as $p) {
    if (!isset($provided[$p->getName()]) || !$provided[$p->getName()]->getConstraint()->matches(new Constraint('=', $p->getVersion()))) {
        $request->fixPackage($p);
    }
}
foreach ($requires as $link) {
    $request->requireName($link->getTarget(), $link->getConstraint());
}

$pool = $repositorySet->createPool($request, $io);

$links = static function (array $links): array {
    $out = [];
    foreach ($links as $target => $link) {
        $out[$target] = $link->getPrettyConstraint();
    }
    return $out;
};
$out = [];
foreach ($pool->getPackages() as $p) {
    $entry = [
        'name' => $p->getName(),
        'version' => $p->getVersion(),
        'pretty' => $p->getPrettyVersion(),
        'repo' => $p->getRepository() === $platformRepo ? 'platform' : ($p->getRepository() instanceof RootPackageRepository ? 'root' : ($p->getRepository() === $lockedRepository ? 'locked' : 'repo')),
        'alias_of' => $p instanceof AliasPackage ? $p->getAliasOf()->getVersion() : null,
        'root_alias' => $p instanceof AliasPackage ? $p->isRootPackageAlias() : null,
        'default_branch' => $p->isDefaultBranch(),
        'stability' => $p->getStability(),
        'dist_ref' => $p->getDistReference(),
        'source_ref' => $p->getSourceReference(),
        'requires' => $links($p->getRequires()),
        'conflicts' => $links($p->getConflicts()),
        'replaces' => $links($p->getReplaces()),
        'provides' => $links($p->getProvides()),
    ];
    $out[] = $entry;
}
echo json_encode(['count' => count($out), 'packages' => $out], JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES), "\n";
