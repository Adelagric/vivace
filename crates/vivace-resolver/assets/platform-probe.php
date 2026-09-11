<?php
// Transcription de Composer\Repository\PlatformRepository::initialize()
// (docs/reference/resolver/PlatformRepository.php) et de Composer\Platform\Version :
// interroge le PHP courant et imprime, dans l'ordre exact de Composer, les
// paquets de plateforme sous forme brute (versions jolies ; la
// normalisation et ses replis sont rejoués côté Rust avec le port exact de
// VersionParser). Les surcharges `config.platform` sont appliquées côté Rust.
//
// Sortie : JSON [{"kind": "php|ext|lib|fixed", "name", "version", "description", "replaces": {}, "provides": {}}]
// Les entrées composer / composer-plugin-api / composer-runtime-api sont
// ajoutées par Rust (constantes de la version de Composer émulée).
error_reporting(0);

function ext_info(string $extension): string {
    try {
        $reflector = new \ReflectionExtension($extension);
    } catch (\Throwable $e) {
        return '';
    }
    ob_start();
    $reflector->info();
    $info = (string) ob_get_clean();
    if ('cli' === PHP_SAPI) {
        return $info;
    }
    return strip_tags(str_replace(['</tr>', '</td>'], ["\n", ' => '], $info));
}

function alpha_to_int(string $alpha): int {
    return strlen($alpha) * (-ord('a') + 1) + array_sum(array_map('ord', str_split($alpha === '' ? "\0" : $alpha))) - ($alpha === '' ? 0 : 0);
}
function convert_alpha(string $alpha): int {
    if ($alpha === '') {
        return 0;
    }
    return strlen($alpha) * (-ord('a') + 1) + array_sum(array_map('ord', str_split($alpha)));
}
function parse_openssl(string $v, &$isFips): ?string {
    $isFips = false;
    if (!preg_match('/^(?<version>[0-9.]+)(?<patch>[a-z]{0,2})(?<suffix>(?:-?(?:dev|pre|alpha|beta|rc|fips)[\d]*)*)(?:-\w+)?(?: \(.+?\))?$/', $v, $m)) {
        return null;
    }
    $patch = '';
    if (version_compare($m['version'], '3.0.0', '<')) {
        $patch = '.' . convert_alpha($m['patch']);
    }
    $isFips = strpos($m['suffix'], 'fips') !== false;
    $suffix = strtr('-' . ltrim($m['suffix'], '-'), ['-fips' => '', '-pre' => '-alpha']);
    return rtrim($m['version'] . $patch . $suffix, '-');
}
function parse_libjpeg(string $v): ?string {
    if (!preg_match('/^(?<major>\d+)(?<minor>[a-z]*)$/', $v, $m)) {
        return null;
    }
    return $m['major'] . '.' . convert_alpha($m['minor']);
}
function parse_zoneinfo(string $v): ?string {
    if (!preg_match('/^(?<year>\d{4})(?<revision>[a-z]*)$/', $v, $m)) {
        return null;
    }
    return $m['year'] . '.' . convert_alpha($m['revision']);
}
function convert_version_id(int $id, int $base): string {
    return sprintf('%d.%d.%d', $id / ($base * $base), (int) ($id / $base) % $base, $id % $base);
}

$out = [];
$php = ['kind' => 'php', 'name' => 'php', 'version' => PHP_VERSION, 'description' => 'The PHP interpreter'];
$out[] = $php;
if (PHP_DEBUG) {
    $out[] = ['kind' => 'php', 'name' => 'php-debug', 'version' => PHP_VERSION, 'description' => 'The PHP interpreter, with debugging symbols'];
}
if (defined('PHP_ZTS') && PHP_ZTS) {
    $out[] = ['kind' => 'php', 'name' => 'php-zts', 'version' => PHP_VERSION, 'description' => 'The PHP interpreter, with Zend Thread Safety'];
}
if (PHP_INT_SIZE === 8) {
    $out[] = ['kind' => 'php', 'name' => 'php-64bit', 'version' => PHP_VERSION, 'description' => 'The PHP interpreter, 64bit'];
}
if (defined('AF_INET6') || @inet_pton('::') !== false) {
    $out[] = ['kind' => 'php', 'name' => 'php-ipv6', 'version' => PHP_VERSION, 'description' => 'The PHP interpreter, with IPv6 support'];
}

$loaded = get_loaded_extensions();
// Composer se relance sans xdebug (composer/xdebug-handler) quand il est
// chargé et actif et que COMPOSER_ALLOW_XDEBUG ne l'autorise pas ; dans le
// processus relancé, ext-xdebug est ajouté après toutes les autres
// extensions (XdebugHandler::getSkippedVersion).
$xdebug_skipped = null;
if (extension_loaded('xdebug')) {
    $xv = phpversion('xdebug');
    $xv = $xv !== false ? $xv : 'unknown';
    if (version_compare($xv, '3.1', '>=')) {
        $modes = xdebug_info('mode');
        $active = count($modes) !== 0;
    } else {
        $ini = ini_get('xdebug.mode');
        if ($ini === false) {
            $active = true;
        } else {
            $env = (string) getenv('XDEBUG_MODE');
            $mode = $env !== '' ? $env : ($ini !== '' ? $ini : 'off');
            if (preg_match('/^,+$/', str_replace(' ', '', $mode))) {
                $mode = 'off';
            }
            $active = $mode !== 'off';
        }
    }
    $allow = explode('|', (string) getenv('COMPOSER_ALLOW_XDEBUG'));
    if ($active && !((bool) $allow[0])) {
        $xdebug_skipped = $xv;
        $loaded = array_values(array_filter($loaded, static fn ($n) => $n !== 'xdebug'));
    }
}
foreach ($loaded as $name) {
    if (in_array($name, ['standard', 'Core'], true)) {
        continue;
    }
    $v = phpversion($name);
    if ($v === false) {
        $v = '0';
    }
    $out[] = ['kind' => 'ext', 'name' => $name, 'version' => $v];
}
if ($xdebug_skipped !== null) {
    $out[] = ['kind' => 'ext', 'name' => 'xdebug', 'version' => $xdebug_skipped];
}

$libs = [];
$add_lib = function (string $name, ?string $pretty, ?string $description = null, array $replaces = [], array $provides = []) use (&$out) {
    if ($pretty === null) {
        return;
    }
    $out[] = ['kind' => 'lib', 'name' => $name, 'version' => $pretty, 'description' => $description, 'replaces' => $replaces, 'provides' => $provides];
};

foreach ($loaded as $name) {
    switch ($name) {
        case 'amqp':
            $info = ext_info($name);
            if (preg_match('/^librabbitmq version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-librabbitmq', $m['version'], 'AMQP librabbitmq version');
            }
            if (preg_match('/^AMQP protocol version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-protocol', str_replace('-', '.', $m['version']), 'AMQP protocol version');
            }
            break;
        case 'bz2':
            $info = ext_info($name);
            if (preg_match('/^BZip2 Version => (?<version>.*),/im', $info, $m)) {
                $add_lib($name, $m['version']);
            }
            break;
        case 'curl':
            $cv = curl_version();
            $add_lib($name, $cv['version']);
            $info = ext_info($name);
            if (preg_match('{^SSL Version => (?<library>[^\r\n/]+)/(?<version>[^\r\n]+?)\r?$}im', $info, $sm)) {
                $library = strtolower($sm['library']);
                if ($library === 'openssl') {
                    $parsed = parse_openssl($sm['version'], $isFips);
                    $add_lib($name . '-openssl' . ($isFips ? '-fips' : ''), $parsed, 'curl OpenSSL version (' . $parsed . ')', [], $isFips ? ['curl-openssl'] : []);
                } else {
                    if (str_starts_with($library, '(securetransport)') && preg_match('{^\(securetransport\) ([a-z0-9]+)}', $library, $stm)) {
                        $shortlib = 'securetransport';
                        $sslLib = 'curl-' . $stm[1];
                    } else {
                        $shortlib = $library;
                        $sslLib = 'curl-openssl';
                    }
                    $add_lib($name . '-' . $shortlib, $sm['version'], 'curl ' . $library . ' version (' . $sm['version'] . ')', [$sslLib]);
                }
            }
            if (preg_match('{^libSSH Version => (?<library>[^\r\n/]+)/(?<version>.+?)(?:/.*)?$}im', $info, $sshm)) {
                $add_lib($name . '-' . strtolower($sshm['library']), $sshm['version'], 'curl ' . $sshm['library'] . ' version');
            }
            if (preg_match('{^ZLib Version => (?<version>.+)$}im', $info, $zm)) {
                $add_lib($name . '-zlib', $zm['version'], 'curl zlib version');
            }
            break;
        case 'date':
            $info = ext_info($name);
            if (preg_match('/^timelib version => (?<version>.+)$/im', $info, $tm)) {
                $add_lib($name . '-timelib', $tm['version'], 'date timelib version');
            }
            if (preg_match('/^Timezone Database => (?<source>internal|external)$/im', $info, $zsm)) {
                $external = $zsm['source'] === 'external';
                if (preg_match('/^"Olson" Timezone Database Version => (?<version>.+?)(?:\.system)?$/im', $info, $zm)) {
                    if ($external && in_array('timezonedb', $loaded, true)) {
                        $add_lib('timezonedb-zoneinfo', $zm['version'], 'zoneinfo ("Olson") database for date (replaced by timezonedb)', [$name . '-zoneinfo']);
                    } else {
                        $add_lib($name . '-zoneinfo', $zm['version'], 'zoneinfo ("Olson") database for date');
                    }
                }
            }
            break;
        case 'fileinfo':
            $info = ext_info($name);
            if (preg_match('/^libmagic => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libmagic', $m['version'], 'fileinfo libmagic version');
            }
            break;
        case 'gd':
            $add_lib($name, defined('GD_VERSION') ? GD_VERSION : null);
            $info = ext_info($name);
            if (preg_match('/^libJPEG Version => (?<version>.+?)(?: compatible)?$/im', $info, $m)) {
                $add_lib($name . '-libjpeg', parse_libjpeg($m['version']), 'libjpeg version for gd');
            }
            if (preg_match('/^libPNG Version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libpng', $m['version'], 'libpng version for gd');
            }
            if (preg_match('/^FreeType Version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-freetype', $m['version'], 'freetype version for gd');
            }
            if (preg_match('/^libXpm Version => (?<versionId>\d+)$/im', $info, $m)) {
                $add_lib($name . '-libxpm', convert_version_id((int) $m['versionId'], 100), 'libxpm version for gd');
            }
            break;
        case 'gmp':
            $add_lib($name, defined('GMP_VERSION') ? GMP_VERSION : null);
            break;
        case 'iconv':
            $add_lib($name, defined('ICONV_VERSION') ? ICONV_VERSION : null);
            break;
        case 'intl':
            $info = ext_info($name);
            $description = 'The ICU unicode and globalization support library';
            if (defined('INTL_ICU_VERSION')) {
                $add_lib('icu', INTL_ICU_VERSION, $description);
            } elseif (preg_match('/^ICU version => (?<version>.+)$/im', $info, $m)) {
                $add_lib('icu', $m['version'], $description);
            }
            if (preg_match('/^ICU TZData version => (?<version>.*)$/im', $info, $zm) && null !== ($zv = parse_zoneinfo($zm['version']))) {
                $add_lib('icu-zoneinfo', $zv, 'zoneinfo ("Olson") database for icu');
            }
            if (class_exists('ResourceBundle')) {
                $rb = @ResourceBundle::create('root', 'ICUDATA', false);
                if ($rb !== null) {
                    $add_lib('icu-cldr', $rb->get('Version'), 'ICU CLDR project version');
                }
            }
            if (class_exists('IntlChar')) {
                $add_lib('icu-unicode', implode('.', array_slice(IntlChar::getUnicodeVersion(), 0, 3)), 'ICU unicode version');
            }
            break;
        case 'imagick':
            $imv = (new Imagick())->getVersion();
            if (preg_match('/^ImageMagick (?<version>[\d.]+)(?:-(?<patch>\d+))?/', $imv['versionString'], $m)) {
                $version = $m['version'];
                if (isset($m['patch'])) {
                    $version .= '.' . $m['patch'];
                }
                $add_lib($name . '-imagemagick', $version, null, ['imagick']);
            }
            break;
        case 'ldap':
            $info = ext_info($name);
            if (preg_match('/^Vendor Version => (?<versionId>\d+)$/im', $info, $m) && preg_match('/^Vendor Name => (?<vendor>.+)$/im', $info, $vm)) {
                $add_lib($name . '-' . strtolower($vm['vendor']), convert_version_id((int) $m['versionId'], 100), $vm['vendor'] . ' version of ldap');
            }
            break;
        case 'libxml':
            $libxmlProvides = array_map(static function ($extension): string {
                return $extension . '-libxml';
            }, array_intersect($loaded, ['dom', 'simplexml', 'xml', 'xmlreader', 'xmlwriter']));
            $add_lib($name, defined('LIBXML_DOTTED_VERSION') ? LIBXML_DOTTED_VERSION : null, 'libxml library version', [], array_values($libxmlProvides));
            break;
        case 'mbstring':
            $info = ext_info($name);
            if (preg_match('/^libmbfl version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libmbfl', $m['version'], 'mbstring libmbfl version');
            }
            if (PHP_VERSION_ID < 90000 && defined('MB_ONIGURUMA_VERSION')) {
                $add_lib($name . '-oniguruma', @constant('MB_ONIGURUMA_VERSION'), 'mbstring oniguruma version');
            } elseif (preg_match('/^(?:oniguruma|Multibyte regex \(oniguruma\)) version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-oniguruma', $m['version'], 'mbstring oniguruma version');
            }
            break;
        case 'memcached':
            $info = ext_info($name);
            if (preg_match('/^libmemcached version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libmemcached', $m['version'], 'libmemcached version');
            }
            break;
        case 'openssl':
            if (preg_match('{^(?:OpenSSL|LibreSSL)?\s*(?<version>\S+)}i', OPENSSL_VERSION_TEXT, $m)) {
                $parsed = parse_openssl($m['version'], $isFips);
                $add_lib($name . ($isFips ? '-fips' : ''), $parsed, OPENSSL_VERSION_TEXT, [], $isFips ? [$name] : []);
            }
            break;
        case 'pcre':
            $add_lib($name, preg_replace('{^(\S+).*}', '$1', PCRE_VERSION));
            $info = ext_info($name);
            if (preg_match('/^PCRE Unicode Version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-unicode', $m['version'], 'PCRE Unicode version support');
            }
            break;
        case 'mysqlnd':
        case 'pdo_mysql':
            $info = ext_info($name);
            if (preg_match('/^(?:Client API version|Version) => mysqlnd (?<version>.+?) /mi', $info, $m)) {
                $add_lib($name . '-mysqlnd', $m['version'], 'mysqlnd library version for ' . $name);
            }
            break;
        case 'mongodb':
            $info = ext_info($name);
            if (preg_match('/^libmongoc bundled version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libmongoc', $m['version'], 'libmongoc version of mongodb');
            }
            if (preg_match('/^libbson bundled version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libbson', $m['version'], 'libbson version of mongodb');
            }
            break;
        case 'pgsql':
            if (defined('PGSQL_LIBPQ_VERSION')) {
                $add_lib('pgsql-libpq', PGSQL_LIBPQ_VERSION, 'libpq for pgsql');
                break;
            }
            // fallthrough
        case 'pdo_pgsql':
            $info = ext_info($name);
            if (preg_match('/^PostgreSQL\(libpq\) Version => (?<version>.*)$/im', $info, $m)) {
                $add_lib($name . '-libpq', $m['version'], 'libpq for ' . $name);
            }
            break;
        case 'pq':
            $info = ext_info($name);
            if (preg_match('/^libpq => (?<compiled>.+) => (?<linked>.+)$/im', $info, $m)) {
                $add_lib($name . '-libpq', $m['linked'], 'libpq for ' . $name);
            }
            break;
        case 'rdkafka':
            if (defined('RD_KAFKA_VERSION')) {
                $i = RD_KAFKA_VERSION;
                $add_lib($name . '-librdkafka', sprintf('%d.%d.%d', ($i & 0x7F000000) >> 24, ($i & 0x00FF0000) >> 16, ($i & 0x0000FF00) >> 8), 'librdkafka for ' . $name);
            }
            break;
        case 'libsodium':
        case 'sodium':
            if (defined('SODIUM_LIBRARY_VERSION')) {
                $add_lib('libsodium', SODIUM_LIBRARY_VERSION);
                $add_lib('libsodium', SODIUM_LIBRARY_VERSION);
            }
            break;
        case 'sqlite3':
        case 'pdo_sqlite':
            $info = ext_info($name);
            if (preg_match('/^SQLite Library => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-sqlite', $m['version']);
            }
            break;
        case 'ssh2':
            $info = ext_info($name);
            if (preg_match('/^libssh2 version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libssh2', $m['version']);
            }
            break;
        case 'xsl':
            $add_lib('libxslt', defined('LIBXSLT_DOTTED_VERSION') ? LIBXSLT_DOTTED_VERSION : null, null, ['xsl']);
            $info = ext_info('xsl');
            if (preg_match('/^libxslt compiled against libxml Version => (?<version>.+)$/im', $info, $m)) {
                $add_lib('libxslt-libxml', $m['version'], 'libxml version libxslt is compiled against');
            }
            break;
        case 'yaml':
            $info = ext_info('yaml');
            if (preg_match('/^LibYAML Version => (?<version>.+)$/im', $info, $m)) {
                $add_lib($name . '-libyaml', $m['version'], 'libyaml version of yaml');
            }
            break;
        case 'zip':
            if (defined('ZipArchive::LIBZIP_VERSION')) {
                $add_lib($name . '-libzip', ZipArchive::LIBZIP_VERSION, null, ['zip']);
            }
            break;
        case 'zlib':
            if (defined('ZLIB_VERSION')) {
                $add_lib($name, ZLIB_VERSION);
            } elseif (preg_match('/^Linked Version => (?<version>.+)$/im', ext_info($name), $m)) {
                $add_lib($name, $m['version']);
            }
            break;
        default:
            break;
    }
}
if (defined('HHVM_VERSION')) {
    $out[] = ['kind' => 'php', 'name' => 'hhvm', 'version' => HHVM_VERSION, 'description' => 'The HHVM Runtime (64bit)'];
}
echo json_encode($out);
