use serde::{Deserialize, Serialize};

/* A place in the document: a page and how far down it, from 0 to 1 */
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct Spot {
    pub page: usize,
    pub within: f32,
}

impl Spot {
    fn near(&self, other: &Spot) -> bool {
        self.page == other.page && (self.within - other.within).abs() < 0.05
    }
}

/* The places jumped away from, like vim's jump list. `index` is where Ctrl-o and
 * Ctrl-n walk; it equals the length while at the newest place. */
#[derive(Debug, Default)]
pub struct Jumps {
    spots: Vec<Spot>,
    index: usize,
}

const MAX_SPOTS: usize = 100;

impl Jumps {
    /* Remembers `from` before a jump. Walking back and then jumping drops the places
     * ahead, as in vim. */
    pub fn record(&mut self, from: Spot) {
        self.spots.truncate(self.index);
        if self.spots.last().is_some_and(|last| last.near(&from)) {
            self.spots.pop();
        }
        self.spots.push(from);
        if self.spots.len() > MAX_SPOTS {
            self.spots.remove(0);
        }
        self.index = self.spots.len();
    }

    /* The place before this one; `here` is kept so that going forward comes back */
    pub fn back(&mut self, here: Spot) -> Option<Spot> {
        if self.index == self.spots.len() {
            if self.spots.last().is_some_and(|last| last.near(&here)) {
                /* Already at the newest place: step past it */
                self.index = self.spots.len() - 1;
            } else {
                self.spots.push(here);
                self.index = self.spots.len() - 1;
            }
        }
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        self.spots.get(self.index).copied()
    }

    pub fn forward(&mut self) -> Option<Spot> {
        if self.index + 1 >= self.spots.len() {
            return None;
        }
        self.index += 1;
        self.spots.get(self.index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(page: usize) -> Spot {
        Spot { page, within: 0.0 }
    }

    #[test]
    fn back_and_forward_walk_the_jumps() {
        let mut jumps = Jumps::default();
        /* 1 → 10 → 20, now reading 20 */
        jumps.record(at(1));
        jumps.record(at(10));
        assert_eq!(jumps.back(at(20)), Some(at(10)));
        assert_eq!(jumps.back(at(10)), Some(at(1)));
        assert_eq!(jumps.back(at(1)), None);
        assert_eq!(jumps.forward(), Some(at(10)));
        assert_eq!(jumps.forward(), Some(at(20)));
        assert_eq!(jumps.forward(), None);
    }

    #[test]
    fn jumping_after_going_back_drops_the_places_ahead() {
        let mut jumps = Jumps::default();
        jumps.record(at(1));
        jumps.record(at(10));
        assert_eq!(jumps.back(at(20)), Some(at(10)));
        /* From 10 jump to 50 */
        jumps.record(at(10));
        assert_eq!(jumps.forward(), None);
        assert_eq!(jumps.back(at(50)), Some(at(10)));
        assert_eq!(jumps.back(at(10)), Some(at(1)));
    }

    #[test]
    fn nearby_places_are_kept_once() {
        let mut jumps = Jumps::default();
        jumps.record(at(3));
        jumps.record(Spot {
            page: 3,
            within: 0.01,
        });
        jumps.record(at(3));
        assert_eq!(jumps.spots.len(), 1);
        /* Going back from where the last jump started skips it */
        assert_eq!(jumps.back(at(3)), None);
    }

    #[test]
    fn nothing_to_go_back_to() {
        let mut jumps = Jumps::default();
        assert_eq!(jumps.back(at(4)), None);
        assert_eq!(jumps.forward(), None);
    }

    #[test]
    fn old_places_are_forgotten() {
        let mut jumps = Jumps::default();
        for page in 0..150 {
            jumps.record(at(page));
        }
        assert_eq!(jumps.spots.len(), MAX_SPOTS);
        assert_eq!(jumps.spots[0], at(50));
    }
}
