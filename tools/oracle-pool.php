<?php
// Oracle R1 : le pool que Composer construit pour `composer update` dans le
// projet courant (composer.json + composer.lock + config globale de
// COMPOSER_HOME), sans optimiseur ni filtres de sécurité — réplique de
// Installer::doUpdate jusqu'à createPool (createRepositorySet, createRequest,
// requirePackagesForUpdate). Sortie JSON sur stdout : les paquets du pool
// dans l'ordre (l'ordre est celui que le solveur verra).
//
// Avec `--solve` (R2) : le pool passe par le PoolOptimizer comme dans
// Installer::doUpdate, puis Solver::solve ; la sortie ajoute les décisions
// dans l'ordre (littéraux du pool optimisé), la taille du jeu de règles,
// les opérations de la LockTransaction et ses paquets pour le lock.
//
// Usage : COMPOSER_HOME=… COMPOSER_ROOT_VERSION=… php tools/oracle-pool.php <composer.phar> [--solve]
declare(strict_types=1);
ini_set("memory_limit", "-1");

$phar = $argv[1] ?? null;
if ($phar === null || !is_file($phar)) {
    fwrite(STDERR, "usage: oracle-pool.php <composer.phar>\n");
    exit(2);
}
require "phar://$phar/vendor/autoload.php";

use Composer\DependencyResolver\Operation\InstallOperation;
use Composer\DependencyResolver\Operation\MarkAliasInstalledOperation;
use Composer\DependencyResolver\Operation\MarkAliasUninstalledOperation;
use Composer\DependencyResolver\Operation\UninstallOperation;
use Composer\DependencyResolver\Operation\UpdateOperation;
use Composer\DependencyResolver\DefaultPolicy;
use Composer\DependencyResolver\PoolOptimizer;
use Composer\DependencyResolver\Request;
use Composer\DependencyResolver\Solver;
use Composer\DependencyResolver\SolverProblemsException;
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

$solve = in_array('--solve', $argv, true);
$policy = new DefaultPolicy($package->getPreferStable(), false, null);
$pool = $repositorySet->createPool($request, $io, null, $solve ? new PoolOptimizer($policy) : null);

// Indexé par la clé PHP du lien (cible en général, nom nu pour les lib-* de
// la plateforme, numérique pour les liens self.version d'un alias) : c'est
// la clé que Pool::match consulte.
$links = static function (array $links): array {
    $out = [];
    foreach ($links as $key => $link) {
        $out[$key] = $link->getPrettyConstraint();
    }
    return $out;
};
$out = [];
foreach ($pool->getPackages() as $p) {
    $entry = [
        'name' => $p->getName(),
        'version' => $p->getVersion(),
        'pretty' => $p->getPrettyVersion(),
        // Un alias racine créé par PoolBuilder::loadPackage n'a pas de dépôt.
        'repo' => $p->getRepository() === null ? 'none' : ($p->getRepository() === $platformRepo ? 'platform' : ($p->getRepository() instanceof RootPackageRepository ? 'root' : ($p->getRepository() === $lockedRepository ? 'locked' : 'repo'))),
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
$result = ['count' => count($out), 'packages' => $out];
if ($solve) {
    $solver = new Solver($policy, $pool, $io);
    try {
        $transaction = $solver->solve($request);
    } catch (SolverProblemsException $e) {
        $result['problems'] = $e->getPrettyString($repositorySet, $request, $pool, false);
        echo json_encode($result, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES), "\n";
        exit(0);
    }
    $ref = new ReflectionProperty(Solver::class, 'decisions');
    $decisions = $ref->getValue($solver);
    $literals = [];
    for ($i = 0; $i < count($decisions); $i++) {
        $literals[] = $decisions->atOffset($i)[0];
    }
    $result['rules'] = $solver->getRuleSetSize();
    $rulesRef = new ReflectionProperty(Solver::class, 'rules');
    $result['learned'] = count($rulesRef->getValue($solver)->getRules()[Composer\DependencyResolver\RuleSet::TYPE_LEARNED]);
    $result['decisions'] = $literals;
    $ops = [];
    foreach ($transaction->getOperations() as $op) {
        if ($op instanceof InstallOperation) {
            $ops[] = ['install', $op->getPackage()->getName(), $op->getPackage()->getVersion()];
        } elseif ($op instanceof UpdateOperation) {
            $ops[] = ['update', $op->getInitialPackage()->getName(), $op->getInitialPackage()->getVersion(), $op->getTargetPackage()->getVersion()];
        } elseif ($op instanceof UninstallOperation) {
            $ops[] = ['uninstall', $op->getPackage()->getName(), $op->getPackage()->getVersion()];
        } elseif ($op instanceof MarkAliasInstalledOperation) {
            $ops[] = ['markAliasInstalled', $op->getPackage()->getName(), $op->getPackage()->getVersion()];
        } elseif ($op instanceof MarkAliasUninstalledOperation) {
            $ops[] = ['markAliasUninstalled', $op->getPackage()->getName(), $op->getPackage()->getVersion()];
        }
    }
    $result['operations'] = $ops;
    $lock = [];
    foreach ($transaction->getNewLockPackages(false) as $p) {
        $lock[] = [$p->getName(), $p->getVersion()];
    }
    $result['lock'] = $lock;
}
echo json_encode($result, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES), "\n";
