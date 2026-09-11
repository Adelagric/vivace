//! Port de `Composer\DependencyResolver\Decisions` : la carte des décisions
//! (paquet → ±niveau) et la file des décisions avec leur règle.

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SolverBug(pub String);

#[derive(Debug, Clone, Copy)]
pub struct Decision {
    pub literal: i64,
    /// Identifiant de la règle (`DECISION_REASON`).
    pub reason: usize,
}

#[derive(Debug, Clone)]
pub struct Decisions {
    /// Indexée par identifiant de pool (1-based ; 0 inutilisé) : 0 =
    /// indécis, > 0 = installé au niveau, < 0 = refusé au niveau.
    map: Vec<i64>,
    pub queue: Vec<Decision>,
}

impl Decisions {
    pub fn new(pool_size: usize) -> Decisions {
        Decisions {
            map: vec![0; pool_size + 1],
            queue: Vec::new(),
        }
    }

    fn entry(&self, literal_or_id: i64) -> i64 {
        let id = literal_or_id.unsigned_abs() as usize;
        self.map.get(id).copied().unwrap_or(0)
    }

    pub fn decide(&mut self, literal: i64, level: i64, reason: usize) -> Result<(), SolverBug> {
        self.add_decision(literal, level)?;
        self.queue.push(Decision { literal, reason });
        Ok(())
    }

    pub fn satisfy(&self, literal: i64) -> bool {
        let d = self.entry(literal);
        (literal > 0 && d > 0) || (literal < 0 && d < 0)
    }

    pub fn conflict(&self, literal: i64) -> bool {
        let d = self.entry(literal);
        (d > 0 && literal < 0) || (d < 0 && literal > 0)
    }

    pub fn decided(&self, literal_or_id: i64) -> bool {
        self.entry(literal_or_id) != 0
    }

    pub fn undecided(&self, literal_or_id: i64) -> bool {
        self.entry(literal_or_id) == 0
    }

    pub fn decided_install(&self, literal_or_id: i64) -> bool {
        self.entry(literal_or_id) > 0
    }

    pub fn decision_level(&self, literal_or_id: i64) -> i64 {
        self.entry(literal_or_id).abs()
    }

    /// `decisionRule` : la règle de la première décision sur ce paquet.
    pub fn decision_rule(&self, literal_or_id: i64) -> Result<usize, SolverBug> {
        let id = literal_or_id.abs();
        self.queue
            .iter()
            .find(|d| d.literal.abs() == id)
            .map(|d| d.reason)
            .ok_or_else(|| {
                SolverBug(format!(
                    "Did not find a decision rule using {literal_or_id}"
                ))
            })
    }

    pub fn at_offset(&self, offset: usize) -> Decision {
        self.queue[offset]
    }

    pub fn valid_offset(&self, offset: usize) -> bool {
        offset < self.queue.len()
    }

    pub fn last_reason(&self) -> usize {
        self.queue[self.queue.len() - 1].reason
    }

    pub fn last_literal(&self) -> i64 {
        self.queue[self.queue.len() - 1].literal
    }

    /// `resetToOffset($offset)` : garde `offset + 1` décisions.
    pub fn reset_to_offset(&mut self, offset: i64) {
        while (self.queue.len() as i64) > offset + 1 {
            let d = self.queue.pop().expect("non-empty");
            self.map[d.literal.unsigned_abs() as usize] = 0;
        }
    }

    pub fn revert_last(&mut self) {
        let last = self.last_literal();
        self.map[last.unsigned_abs() as usize] = 0;
        self.queue.pop();
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn add_decision(&mut self, literal: i64, level: i64) -> Result<(), SolverBug> {
        let id = literal.unsigned_abs() as usize;
        if id >= self.map.len() {
            return Err(SolverBug(format!("literal {literal} out of pool")));
        }
        let previous = self.map[id];
        if previous != 0 {
            return Err(SolverBug(format!(
                "Trying to decide {literal} on level {level}, even though package {id} was previously decided as {previous}."
            )));
        }
        self.map[id] = if literal > 0 { level } else { -level };
        Ok(())
    }
}
