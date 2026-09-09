//! Port de `Composer\Util\PackageSorter::sortPackages` : chaque paquet reçoit
//! un poids d'après ses « utilisateurs » (qui le requiert), récursivement ; les
//! plus requis remontent en tête. Égalité départagée par strnatcasecmp.

use crate::natsort::strnatcasecmp;
use std::collections::BTreeMap;

/// Un paquet vu par le trieur : nom + cibles de ses `require` (+ `require-dev`
/// pour la racine).
pub struct SortablePackage<'a> {
    pub name: &'a str,
    pub requires: Vec<&'a str>,
}

/// Renvoie les indices des paquets dans l'ordre trié.
pub fn sort_packages(packages: &[SortablePackage<'_>]) -> Vec<usize> {
    let mut usage: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in packages {
        for target in &p.requires {
            usage.entry(target).or_default().push(p.name);
        }
    }

    struct Ctx<'a> {
        usage: &'a BTreeMap<&'a str, Vec<&'a str>>,
        computing: std::collections::BTreeSet<&'a str>,
        computed: BTreeMap<&'a str, i64>,
    }
    fn importance<'a>(ctx: &mut Ctx<'a>, name: &'a str) -> i64 {
        if let Some(w) = ctx.computed.get(name) {
            return *w;
        }
        if ctx.computing.contains(name) {
            return 0;
        }
        ctx.computing.insert(name);
        let mut weight: i64 = 0;
        if let Some(users) = ctx.usage.get(name) {
            let users = users.clone();
            for user in users {
                weight -= 1 - importance(ctx, user);
            }
        }
        ctx.computing.remove(name);
        ctx.computed.insert(name, weight);
        weight
    }

    let mut ctx = Ctx {
        usage: &usage,
        computing: Default::default(),
        computed: Default::default(),
    };
    let mut weighted: Vec<(i64, usize)> = packages
        .iter()
        .enumerate()
        .map(|(i, p)| (importance(&mut ctx, p.name), i))
        .collect();
    weighted.sort_by(|(wa, ia), (wb, ib)| {
        wa.cmp(wb)
            .then_with(|| strnatcasecmp(packages[*ia].name, packages[*ib].name))
    });
    weighted.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_required_first_then_natural_name() {
        let pk = |name, req: &[&'static str]| SortablePackage {
            name,
            requires: req.to_vec(),
        };
        let packages = vec![
            pk("app/z", &["lib/a", "lib/b"]),
            pk("lib/a", &["lib/base"]),
            pk("lib/b", &["lib/base"]),
            pk("lib/base", &[]),
            pk("lib/c10", &[]),
            pk("lib/c9", &[]),
        ];
        let order: Vec<&str> = sort_packages(&packages)
            .into_iter()
            .map(|i| packages[i].name)
            .collect();
        // base est requis par a et b (chacun requis par z) → poids le plus négatif.
        assert_eq!(order[0], "lib/base");
        assert_eq!(&order[1..3], &["lib/a", "lib/b"]);
        // Poids 0 : app/z, c9 avant c10 (naturel).
        assert_eq!(&order[3..], &["app/z", "lib/c9", "lib/c10"]);
    }
}
