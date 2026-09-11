#!/usr/bin/env bash
# Drift, premier étage : chaque fichier de docs/reference/ est la copie d'un
# fichier du phar Composer 2.10.3 (ou d'une dépendance embarquée). On extrait
# les mêmes fichiers du Composer installé et on diffe : un fichier qui a bougé
# désigne exactement le port à revoir, avant même de lancer le harness.
#
# Usage : harness/drift-reference.sh [composer.phar]   (défaut : `which composer`)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PHAR="${1:-$(command -v composer)}"
[ -f "$PHAR" ] || { echo "composer introuvable"; exit 1; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
cp "$PHAR" "$TMP/composer.phar"

# docs/reference/<fichier> → chemin dans le phar
twin() {
  case "$1" in
    ArrayDumper.php)          echo "src/Composer/Package/Dumper/ArrayDumper.php" ;;
    AutoloadGenerator.php)    echo "src/Composer/Autoload/AutoloadGenerator.php" ;;
    BinaryInstaller.php)      echo "src/Composer/Installer/BinaryInstaller.php" ;;
    ClassLoader.php)          echo "src/Composer/Autoload/ClassLoader.php" ;;
    Factory.php)              echo "src/Composer/Factory.php" ;;
    FilesystemRepository.php) echo "src/Composer/Repository/FilesystemRepository.php" ;;
    InstalledVersions.php)    echo "src/Composer/InstalledVersions.php" ;;
    JsonFile.php)             echo "src/Composer/Json/JsonFile.php" ;;
    Locker.php)               echo "src/Composer/Package/Locker.php" ;;
    PackageSorter.php)        echo "src/Composer/Util/PackageSorter.php" ;;
    RootPackageLoader.php)    echo "src/Composer/Package/Loader/RootPackageLoader.php" ;;
    VersionGuesser.php)       echo "src/Composer/Package/Version/VersionGuesser.php" ;;
    Filesystem.php)           echo "src/Composer/Util/Filesystem.php" ;;
    PluginManager.php)        echo "src/Composer/Plugin/PluginManager.php" ;;
    ArchiveDownloader.php)    echo "src/Composer/Downloader/ArchiveDownloader.php" ;;
    LibraryInstaller.php)     echo "src/Composer/Installer/LibraryInstaller.php" ;;
    SemverVersionParser.php)  echo "vendor/composer/semver/src/VersionParser.php" ;;
    cmg-ClassMap.php)         echo "vendor/composer/class-map-generator/src/ClassMap.php" ;;
    cmg-ClassMapGenerator.php) echo "vendor/composer/class-map-generator/src/ClassMapGenerator.php" ;;
    cmg-FileList.php)         echo "vendor/composer/class-map-generator/src/FileList.php" ;;
    cmg-PhpFileCleaner.php)   echo "vendor/composer/class-map-generator/src/PhpFileCleaner.php" ;;
    cmg-PhpFileParser.php)    echo "vendor/composer/class-map-generator/src/PhpFileParser.php" ;;
    Installer.php)            echo "src/Composer/Installer.php" ;;
    *) echo "" ;;
  esac
}

# docs/reference/resolver/<fichier> → chemin dans le phar : le fichier est
# cherché sous les espaces de noms que le résolveur touche ; `semver-X.php`
# est composer/semver (src/ ou src/Constraint/), `Operation/X.php` les
# opérations du solveur.
resolver_twin() {
  local name="$1" base cands c
  case "$name" in
    Operation/*) echo "src/Composer/DependencyResolver/$name"; return ;;
    semver-*)
      base="${name#semver-}"
      cands="vendor/composer/semver/src/$base vendor/composer/semver/src/Constraint/$base" ;;
    *)
      cands="src/Composer/DependencyResolver/$name src/Composer/Repository/$name src/Composer/Package/$name src/Composer/Package/Loader/$name src/Composer/Package/Version/$name src/Composer/Filter/PlatformRequirementFilter/$name vendor/composer/metadata-minifier/src/$name" ;;
  esac
  for c in $cands; do
    if php -r 'exit(@file_get_contents("phar://'"$TMP"'/composer.phar/'"$c"'") === false ? 1 : 0);' 2>/dev/null; then
      echo "$c"; return
    fi
  done
  echo ""
}

version=$(php "$TMP/composer.phar" --version --no-ansi 2>/dev/null | sed -n 's/^Composer version \([^ ]*\).*/\1/p')
echo "Composer $version vs docs/reference (2.10.3)"
status=0; checked=0
for f in "$ROOT"/docs/reference/*.php "$ROOT"/docs/reference/resolver/*.php "$ROOT"/docs/reference/resolver/Operation/*.php; do
  name=$(basename "$f")
  [ -s "$f" ] || continue
  case "$f" in
    */docs/reference/resolver/Operation/*) inner=$(resolver_twin "Operation/$name") ;;
    */docs/reference/resolver/*) inner=$(resolver_twin "$name") ;;
    *) inner=$(twin "$name") ;;
  esac
  [ -n "$inner" ] || { echo "??   $name : pas de jumeau connu (à ajouter dans twin())"; status=1; continue; }
  if ! php -r 'echo file_get_contents("phar://'"$TMP"'/composer.phar/'"$inner"'");' > "$TMP/twin.php" 2>/dev/null; then
    echo "MISSING $name : $inner absent du phar"; status=1; continue
  fi
  if diff -q "$f" "$TMP/twin.php" >/dev/null; then
    checked=$((checked + 1))
  else
    echo "DRIFT $name ($inner) : $(diff "$f" "$TMP/twin.php" | grep -c '^[<>]') lignes"
    status=1
  fi
done
echo "$checked fichiers identiques"
exit $status
