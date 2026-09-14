<?php
// Oracle VersionSelector : ce que `composer require <name>` (sans
// contrainte) choisirait dans le projet courant — RepositorySet réduit à
// minimum-stability sur le CompositeRepository [plateforme, dépôts du
// projet], stabilité préférée de RequireCommand, filtre de plateforme du
// projet — pour chaque nom lu sur stdin (JSON : liste de chaînes).
// Sortie JSON : nom → {found, name, version, pretty_version, recommended,
// warnings (mode normal), found_any_stability} ; le dépôt local est
// injecté par COMPOSER_HOME/config.json.
//
// Usage : COMPOSER_HOME=… php tools/oracle-version-selector.php <composer.phar> [--ignore-platform-reqs | --ignore-platform-req=X …]

ini_set('memory_limit', '-1');
$phar = $argv[1] ?? null;
if ($phar === null || !is_file($phar)) {
    fwrite(STDERR, "usage: oracle-version-selector.php <composer.phar> [--ignore-platform-reqs|--ignore-platform-req=X]\n");
    exit(2);
}
$ignoreAll = false;
$ignoreList = [];
foreach (array_slice($argv, 2) as $arg) {
    if ($arg === '--ignore-platform-reqs') {
        $ignoreAll = true;
    } elseif (strpos($arg, '--ignore-platform-req=') === 0) {
        $ignoreList[] = substr($arg, strlen('--ignore-platform-req='));
    }
}
require "phar://$phar/vendor/autoload.php";

use Composer\Factory;
use Composer\Filter\PlatformRequirementFilter\PlatformRequirementFilterFactory;
use Composer\IO\BufferIO;
use Composer\Package\Version\VersionParser;
use Composer\Package\Version\VersionSelector;
use Composer\Repository\CompositeRepository;
use Composer\Repository\PlatformRepository;
use Composer\Repository\RepositorySet;

$io = new BufferIO();
$composer = Factory::create($io, null, true);
$repos = $composer->getRepositoryManager()->getRepositories();
$platformOverrides = $composer->getConfig()->get('platform');
$platformRepo = new PlatformRepository([], $platformOverrides);
$composite = new CompositeRepository(array_merge([$platformRepo], $repos));

$manifest = json_decode((string) file_get_contents('composer.json'), true);
$minimumStability = isset($manifest['minimum-stability'])
    ? VersionParser::normalizeStability($manifest['minimum-stability'])
    : 'stable';
$repoSet = new RepositorySet($minimumStability);
$repoSet->addRepository($composite);
$selector = new VersionSelector($repoSet, $platformRepo);
$preferredStability = $composer->getPackage()->getPreferStable()
    ? 'stable'
    : $composer->getPackage()->getMinimumStability();
$filter = $ignoreAll
    ? PlatformRequirementFilterFactory::ignoreAll()
    : ($ignoreList ? PlatformRequirementFilterFactory::fromBoolOrList($ignoreList) : PlatformRequirementFilterFactory::ignoreNothing());

$names = json_decode(stream_get_contents(STDIN), true);
$out = [];
foreach ($names as $name) {
    $warnIo = new BufferIO();
    $package = $selector->findBestCandidate($name, null, $preferredStability, $filter, 0, $warnIo);
    // Le BufferIO ne connaît pas les styles de Composer : les balises
    // `<warning>…</>` restent dans le texte, on les retire.
    $warnings = array_values(array_filter(array_map(static function (string $line): string {
        return trim(str_replace(['<warning>', '</>'], '', $line));
    }, explode("\n", $warnIo->getOutput())), 'strlen'));
    // Toutes stabilités : les fichiers `~dev` absents de l'instantané
    // (jamais chargés par l'update qui l'a capturé) rendent `null`.
    try {
        $any = $selector->findBestCandidate($name, null, $preferredStability, $filter, RepositorySet::ALLOW_UNACCEPTABLE_STABILITIES) !== false;
    } catch (\Composer\Downloader\TransportException $e) {
        $any = null;
    }
    $entry = [
        'found' => $package !== false,
        'warnings' => $warnings,
        'found_any_stability' => $any,
    ];
    if ($package !== false) {
        $entry['name'] = $package->getPrettyName();
        $entry['version'] = $package->getVersion();
        $entry['pretty_version'] = $package->getPrettyVersion();
        $entry['recommended'] = $selector->findRecommendedRequireVersion($package);
    }
    $out[$name] = $entry;
}
echo json_encode($out, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE);
