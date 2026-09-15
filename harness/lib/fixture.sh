# Mise en place d'une copie de fixture pour les harnais : `composer.json` et
# `composer.lock` pour les projets ordinaires ; pour `path-repos`, l'arbre
# entier matérialisé par fixtures/path-repos.sh (dépôts git imbriqués, liens,
# modes, dépôt du projet sur `develop`).
#
# Environnement git des deux côtés : le plafond de recherche est le PARENT du
# répertoire du projet (git n'examine jamais le plafond lui-même : le dépôt
# du projet doit rester visible depuis packages/*, celui du checkout de
# vivacity ne doit jamais l'être), configuration globale et système ignorées
# (signature, tri des branches…), locale C pour l'ordre d'un glob.

# stage_project <fixture> <destination>
stage_project() {
  local fx="$1" dest="$2"
  rm -rf "$dest"; mkdir -p "$dest"
  if [ "$fx" = "path-repos" ]; then
    "$ROOT/fixtures/path-repos.sh" "$dest"
  else
    cp "$ROOT/fixtures/projects/$fx/composer.json" "$dest/"
    [ -f "$ROOT/fixtures/projects/$fx/composer.lock" ] && cp "$ROOT/fixtures/projects/$fx/composer.lock" "$dest/"
  fi
  return 0
}

# harness_git_env <parent of the project directories>
harness_git_env() {
  export GIT_CEILING_DIRECTORIES="$1" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C
}
