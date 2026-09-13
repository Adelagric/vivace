<?php
// Oracle JsonManipulator : lit sur stdin un JSON
//   {"phar": "...", "files": [{"contents": "...", "scenarios": [[[method, args...], ...], ...]}]}
// et rejoue chaque scénario sur un JsonManipulator neuf construit à partir
// de `contents`. Sortie JSON sur stdout, un élément par fichier :
//   {"scenarios": [{"results": [...], "contents": "...", "error": null|"..."}]}
// Une exception (constructeur compris) arrête le scénario et se retrouve
// dans `error` ; les retours des méthodes sont dans `results`.
// Les valeurs passées à addSubNode/addMainKey sont décodées en tableaux
// associatifs ; un objet réduit à la clé "\0stdClass" devient un ArrayObject.

ini_set('memory_limit', '-1');
$input = json_decode(stream_get_contents(STDIN), true);
require "phar://{$input['phar']}/vendor/autoload.php";

use Composer\Json\JsonManipulator;

function toPhp($v)
{
    if (is_array($v)) {
        if (count($v) === 1 && array_key_exists("\0stdClass", $v)) {
            return new \ArrayObject();
        }
        foreach ($v as $k => $x) {
            $v[$k] = toPhp($x);
        }
    }

    return $v;
}

$out = [];
foreach ($input['files'] as $file) {
    $scenarios = [];
    foreach ($file['scenarios'] as $ops) {
        $results = [];
        $error = null;
        $contents = null;
        try {
            $m = new JsonManipulator($file['contents']);
            foreach ($ops as $op) {
                $method = array_shift($op);
                switch ($method) {
                    case 'addLink':
                        $results[] = $m->addLink($op[0], $op[1], $op[2], $op[3] ?? false);
                        break;
                    case 'addSubNode':
                        $results[] = $m->addSubNode($op[0], $op[1], toPhp($op[2]), $op[3] ?? true);
                        break;
                    case 'removeSubNode':
                        $results[] = $m->removeSubNode($op[0], $op[1]);
                        break;
                    case 'addMainKey':
                        $results[] = $m->addMainKey($op[0], toPhp($op[1]));
                        break;
                    case 'removeMainKey':
                        $results[] = $m->removeMainKey($op[0]);
                        break;
                    case 'removeMainKeyIfEmpty':
                        $results[] = $m->removeMainKeyIfEmpty($op[0]);
                        break;
                    case 'removeConfigSetting':
                        $results[] = $m->removeConfigSetting($op[0]);
                        break;
                    case 'format':
                        $results[] = $m->format(toPhp($op[0]), $op[1] ?? 0, $op[2] ?? false);
                        break;
                    default:
                        throw new \RuntimeException('unknown method ' . $method);
                }
            }
            $contents = $m->getContents();
        } catch (\Throwable $e) {
            $error = get_class($e) . ': ' . $e->getMessage();
        }
        $scenarios[] = ['results' => $results, 'contents' => $contents, 'error' => $error];
    }
    $out[] = ['scenarios' => $scenarios];
}

echo json_encode($out, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_INVALID_UTF8_SUBSTITUTE);
