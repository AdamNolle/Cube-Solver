//! Stage 2 — the two-phase coordinates.
//!
//! The two-phase search never works on a whole [`CubieCube`]; it works on small
//! integer *coordinates* so that moves and pruning become table lookups.
//!
//! Phase 1 drives three coordinates to zero (reaching the ⟨U,D,L2,R2,F2,B2⟩ subgroup):
//! - **twist**  — corner orientation, `0..2187` (3⁷)
//! - **flip**   — edge orientation, `0..2048` (2¹¹)
//! - **slice**  — which four slots hold the UD-slice edges (FR,FL,BL,BR), `0..495` (C(12,4))
//!
//! Phase 2 then solves within the subgroup using three permutation coordinates:
//! - **corner_perm**     — `0..40320` (8!)
//! - **edge8_perm**      — the eight U/D edges among slots 0..8, `0..40320` (8!)
//! - **slice_perm**      — the four slice edges among slots 8..12, `0..24` (4!)

use super::CubieCube;

const fn factorial(n: u32) -> u32 {
    let mut f = 1u32;
    let mut i = 2u32;
    while i <= n {
        f *= i;
        i += 1;
    }
    f
}

/// Binomial coefficient C(n, k) (0 for k > n).
const fn binom(n: u32, k: u32) -> u32 {
    if k > n {
        return 0;
    }
    let k = if k > n - k { n - k } else { k };
    let mut num = 1u32;
    let mut den = 1u32;
    let mut i = 0u32;
    while i < k {
        num *= n - i;
        den *= i + 1;
        i += 1;
    }
    num / den
}

// ---- orientation coordinates -------------------------------------------------

/// Corner orientation as a base-3 number over the first 7 corners (`0..2187`).
pub fn twist(c: &CubieCube) -> u16 {
    let mut t = 0u16;
    for i in 0..7 {
        t = t * 3 + c.co[i] as u16;
    }
    t
}

/// Inverse of [`twist`]: set `co` (the 8th corner is forced by the others).
pub fn set_twist(c: &mut CubieCube, mut t: u16) {
    let mut parity = 0u8;
    for i in (0..7).rev() {
        let v = (t % 3) as u8;
        t /= 3;
        c.co[i] = v;
        parity = (parity + v) % 3;
    }
    c.co[7] = (3 - parity) % 3;
}

/// Edge orientation as a base-2 number over the first 11 edges (`0..2048`).
pub fn flip(c: &CubieCube) -> u16 {
    let mut f = 0u16;
    for i in 0..11 {
        f = f * 2 + c.eo[i] as u16;
    }
    f
}

/// Inverse of [`flip`]: set `eo` (the 12th edge is forced by the others).
pub fn set_flip(c: &mut CubieCube, mut f: u16) {
    let mut parity = 0u8;
    for i in (0..11).rev() {
        let v = (f % 2) as u8;
        f /= 2;
        c.eo[i] = v;
        parity ^= v;
    }
    c.eo[11] = parity;
}

// ---- UD-slice combination coordinate ----------------------------------------

/// Which four slots hold the UD-slice edges (FR..BR, ids 8..12), ignoring order
/// (`0..495`). The combinatorial number system over the slice-edge slots.
pub fn slice(c: &CubieCube) -> u16 {
    let mut idx = 0u32;
    let mut k = 0u32; // how many slice edges seen so far (1-based weight)
    for j in 0..12u32 {
        if c.ep[j as usize] >= 8 {
            k += 1;
            idx += binom(j, k);
        }
    }
    idx as u16
}

/// Inverse of [`slice`]: place the four slice edges (FR,FL,BL,BR) into the slots
/// selected by `idx` and the eight U/D edges into the rest (canonical order).
pub fn set_slice(c: &mut CubieCube, idx: u16) {
    let mut chosen = [false; 12];
    let mut rem = idx as u32;
    let mut k = 4u32;
    for j in (0..12u32).rev() {
        if rem >= binom(j, k) {
            rem -= binom(j, k);
            chosen[j as usize] = true;
            k -= 1;
        }
    }
    let mut slice_id = 8u8; // FR,FL,BL,BR
    let mut ud_id = 0u8; // UR..DB
    for (j, &is_slice) in chosen.iter().enumerate() {
        if is_slice {
            c.ep[j] = slice_id;
            slice_id += 1;
        } else {
            c.ep[j] = ud_id;
            ud_id += 1;
        }
    }
}

// ---- permutation coordinates (phase 2) --------------------------------------

fn perm_rank(p: &[u8]) -> u32 {
    let n = p.len();
    let mut rank = 0u32;
    for i in 0..n {
        let mut smaller = 0u32;
        for j in (i + 1)..n {
            if p[j] < p[i] {
                smaller += 1;
            }
        }
        rank += smaller * factorial((n - 1 - i) as u32);
    }
    rank
}

fn perm_unrank(mut rank: u32, n: usize, into: &mut [u8], base: u8) {
    let mut avail: Vec<u8> = (0..n as u8).collect();
    for slot in into.iter_mut().take(n) {
        let fac = factorial((avail.len() - 1) as u32);
        let i = (rank / fac) as usize;
        rank %= fac;
        *slot = avail.remove(i) + base;
    }
}

/// Corner permutation as a Lehmer-code rank (`0..40320`).
pub fn corner_perm(c: &CubieCube) -> u16 {
    perm_rank(&c.cp) as u16
}

pub fn set_corner_perm(c: &mut CubieCube, idx: u16) {
    perm_unrank(idx as u32, 8, &mut c.cp, 0);
}

/// Permutation of the eight U/D edges (slots 0..8, ids 0..8) as a rank (`0..40320`).
pub fn edge8_perm(c: &CubieCube) -> u16 {
    perm_rank(&c.ep[0..8]) as u16
}

pub fn set_edge8_perm(c: &mut CubieCube, idx: u16) {
    let mut tmp = [0u8; 8];
    perm_unrank(idx as u32, 8, &mut tmp, 0);
    c.ep[0..8].copy_from_slice(&tmp);
}

/// Permutation of the four slice edges (slots 8..12, ids 8..12) as a rank (`0..24`).
pub fn slice_perm(c: &CubieCube) -> u8 {
    let mapped: Vec<u8> = c.ep[8..12].iter().map(|&e| e - 8).collect();
    perm_rank(&mapped) as u8
}

pub fn set_slice_perm(c: &mut CubieCube, idx: u8) {
    let mut tmp = [0u8; 4];
    perm_unrank(idx as u32, 4, &mut tmp, 8);
    c.ep[8..12].copy_from_slice(&tmp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solved_coordinates_are_zero() {
        let c = CubieCube::SOLVED;
        assert_eq!(twist(&c), 0);
        assert_eq!(flip(&c), 0);
        assert_eq!(corner_perm(&c), 0);
        assert_eq!(edge8_perm(&c), 0);
        assert_eq!(slice_perm(&c), 0);
        // Solved slice = slice edges in the last four slots = the max combination.
        assert_eq!(
            slice(&c) as u32,
            binom(8, 1) + binom(9, 2) + binom(10, 3) + binom(11, 4)
        );
    }

    #[test]
    fn twist_round_trips_over_full_range() {
        for t in 0..2187u16 {
            let mut c = CubieCube::SOLVED;
            set_twist(&mut c, t);
            assert_eq!(twist(&c), t, "twist {t} round-trip");
            assert_eq!(c.co.iter().map(|&x| x as u32).sum::<u32>() % 3, 0);
        }
    }

    #[test]
    fn flip_round_trips_over_full_range() {
        for f in 0..2048u16 {
            let mut c = CubieCube::SOLVED;
            set_flip(&mut c, f);
            assert_eq!(flip(&c), f, "flip {f} round-trip");
            assert_eq!(c.eo.iter().map(|&x| x as u32).sum::<u32>() % 2, 0);
        }
    }

    #[test]
    fn slice_round_trips_over_full_range() {
        let mut seen = std::collections::HashSet::new();
        for s in 0..495u16 {
            let mut c = CubieCube::SOLVED;
            set_slice(&mut c, s);
            assert_eq!(slice(&c), s, "slice {s} round-trip");
            // exactly four slice edges placed
            assert_eq!(c.ep.iter().filter(|&&e| e >= 8).count(), 4);
            seen.insert(s);
        }
        assert_eq!(seen.len(), 495);
    }

    #[test]
    fn permutation_coords_round_trip() {
        for &idx in &[0u16, 1, 100, 5000, 40319] {
            let mut c = CubieCube::SOLVED;
            set_corner_perm(&mut c, idx);
            assert_eq!(corner_perm(&c), idx, "corner_perm {idx}");
            let mut c = CubieCube::SOLVED;
            set_edge8_perm(&mut c, idx);
            assert_eq!(edge8_perm(&c), idx, "edge8_perm {idx}");
        }
        for idx in 0..24u8 {
            let mut c = CubieCube::SOLVED;
            set_slice_perm(&mut c, idx);
            assert_eq!(slice_perm(&c), idx, "slice_perm {idx}");
        }
    }
}
