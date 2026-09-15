#!/usr/bin/env bash
# Matérialise la fixture `path-repos` dans un répertoire : les sources
# commitées (fixtures/projects/path-repos) plus ce que git ne versionne pas —
# les dépôts git imbriqués (hashes stables : auteur, dates et configuration
# figés), les liens symboliques (dont un pendu et un sortant), les modes 0600
# et 0755, le dossier vide, le fichier `.git` d'un sous-dossier — et le dépôt
# git du projet lui-même sur la branche `develop`, d'où Composer devine la
# version des paquets sans dépôt propre (`dev-develop`, distinct du repli
# `dev-main`).
#
# Usage : fixtures/path-repos.sh <destination>
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
dest="${1:?destination}"
mkdir -p "$dest"
(cd "$DIR/projects/path-repos" && tar -cf - .) | (cd "$dest" && tar -xf -)

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=vivacity GIT_AUTHOR_EMAIL=v@v GIT_COMMITTER_NAME=vivacity GIT_COMMITTER_EMAIL=v@v
commit() { # dir date message
  (cd "$1" && GIT_AUTHOR_DATE="$2" GIT_COMMITTER_DATE="$2" git add -A >/dev/null 2>&1 && \
    GIT_AUTHOR_DATE="$2" GIT_COMMITTER_DATE="$2" git commit -q -m "$3")
}

e="$dest/libs/epsilon"
mkdir -p "$e/empty" "$e/sub/.svn"
printf 'Excluded: .svn is a VCS directory at any depth.\n' > "$e/sub/.svn/entries"
printf 'gitdir: /nowhere\n' > "$e/sub/.git"
chmod 0600 "$e/secret.txt"
chmod 0755 "$e/bin/tool"
ln -s src/Epsilon.php "$e/link-file"
ln -s empty "$e/link-empty"
ln -s src "$e/link-dir"
ln -s missing.txt "$e/link-dangling"
ln -s ../../packages/alpha/composer.json "$e/link-out"
# Its own repository: the HEAD hash becomes the dist reference and the
# version is guessed from its branch (`main`), not the project's.
(cd "$e" && git init -q -b main)
commit "$e" "2026-09-01T10:00:00Z" "epsilon"

# gamma: `main` then a `feature-x` branch checked out — Composer loads the
# feature package and its parent (`dev-feature-x`, `dev-main`).
g="$dest/packages/gamma"
(cd "$g" && git init -q -b main)
commit "$g" "2026-09-01T11:00:00Z" "gamma main"
(cd "$g" && git checkout -q -b feature-x)
printf '<?php\n\nnamespace Acme\\Gamma;\n\nfinal class Feature\n{\n}\n' > "$g/src/Feature.php"
commit "$g" "2026-09-01T12:00:00Z" "gamma feature"

# The project: `develop`, everything committed (the nested repositories
# become gitlinks; `vendor/` is ignored so an install never dirties it).
printf '/vendor/\n' > "$dest/.gitignore"
(cd "$dest" && git init -q -b develop)
commit "$dest" "2026-09-02T00:00:00Z" "project"
