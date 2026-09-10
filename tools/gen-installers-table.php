<?php
// Generates the framework table of composer/installers from the vendored
// reference (docs/reference/installers/src), as JSON on stdout:
//   [{"key": "wordpress", "class": "WordPressInstaller", "custom": false,
//     "locations": {"plugin": "wp-content/plugins/{$name}/", ...}}, ...]
// `custom` is true when the installer overrides inflectPackageVars,
// getLocations or getInstallPath — its paths depend on code vivace does not
// port, so such packages must fall back to Composer.
// Usage: php tools/gen-installers-table.php <composer.phar> [<installers src dir>] [<tag>]
// (default src dir: docs/reference/installers/src/Composer/Installers, tag v2.3.0)
// Output: {"tag": ..., "frameworks": [...]} — written to
// crates/vivace-core/assets/installers/<tag>.json, one file per ported tag.
declare(strict_types=1);

$phar = $argv[1] ?? null;
if ($phar === null || !is_file($phar)) {
    fwrite(STDERR, "usage: gen-installers-table.php <composer.phar>\n");
    exit(2);
}
require "phar://$phar/vendor/autoload.php";
$ref = $argv[2] ?? __DIR__ . '/../docs/reference/installers/src/Composer/Installers';
if (!is_dir($ref)) {
    fwrite(STDERR, "installers src dir not found: $ref\n");
    exit(2);
}
spl_autoload_register(function (string $class) use ($ref): void {
    if (str_starts_with($class, 'Composer\\Installers\\')) {
        $file = $ref . '/' . substr($class, strlen('Composer\\Installers\\')) . '.php';
        if (is_file($file)) {
            require_once $file;
        }
    }
});

$composer = \Composer\Factory::create(new \Composer\IO\NullIO(), ['name' => 'gen/root'], true);
$installer = new \Composer\Installers\Installer(new \Composer\IO\NullIO(), $composer);
$prop = new \ReflectionProperty($installer, 'supportedTypes');
$supported = $prop->getValue($installer);

$base = new \ReflectionClass(\Composer\Installers\BaseInstaller::class);
$out = [];
foreach ($supported as $key => $class) {
    $fqcn = 'Composer\\Installers\\' . $class;
    $rc = new \ReflectionClass($fqcn);
    $custom = false;
    foreach (['inflectPackageVars', 'getLocations', 'getInstallPath', 'templatePath', 'mapCustomInstallPaths'] as $m) {
        if ($rc->getMethod($m)->getDeclaringClass()->getName() !== $base->getName()) {
            $custom = true;
        }
    }
    $pkg = new \Composer\Package\Package('dummy/pkg', '1.0.0.0', '1.0.0');
    $obj = new $fqcn($pkg, $composer, new \Composer\IO\NullIO());
    $locations = $obj->getLocations((string) $key);
    ksort($locations, SORT_STRING);
    $out[] = ['key' => (string) $key, 'class' => $class, 'custom' => $custom, 'locations' => $locations];
}
usort($out, fn($a, $b) => strcmp($a['key'], $b['key']));
echo json_encode(['tag' => $argv[3] ?? 'v2.3.0', 'frameworks' => $out], JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES), "\n";
