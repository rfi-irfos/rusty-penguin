use crate::TernaryTensor;
use ternary_core::Trit;
use alloc::vec::Vec;
use alloc::vec;

/// Sum-aggregate each node's neighbor embeddings, with saturation at [-1,+1]
/// per dimension (via the same step-function convention KindomGNN's own
/// TernaryLinear::forward uses: sum>0 -> Pos, sum<0 -> Neg, else Zero).
/// This is NOT degree-normalized mean -- ternary integer arithmetic has no
/// clean division. Divergence from a float degree-normalized GCN is expected
/// and must be measured (Task 7), not assumed negligible.
pub fn propagate(node_embeddings: &[TernaryTensor], edges: &[(u32, u32)]) -> Vec<TernaryTensor> {
    let n = node_embeddings.len();
    if n == 0 {
        return Vec::new();
    }
    let dim = node_embeddings[0].shape[0];
    let mut sums: Vec<Vec<i32>> = vec![vec![0i32; dim]; n];
    for &(src, dst) in edges {
        let (s, d) = (src as usize, dst as usize);
        if s >= n || d >= n {
            continue;
        }
        for k in 0..dim {
            sums[d][k] += node_embeddings[s].data[k].to_i8() as i32;
        }
    }
    sums.into_iter()
        .map(|row| {
            let trits: Vec<Trit> = row
                .into_iter()
                .map(|v| if v > 0 { Trit::Pos } else if v < 0 { Trit::Neg } else { Trit::Zero })
                .collect();
            TernaryTensor::new(trits, vec![dim])
        })
        .collect()
}

/// DistMult trilinear score: sum(node_a * rel * node_b), elementwise.
/// An INFERENCE score -- callers must not treat this as an observed fact.
pub fn distmult_score(node_a: &TernaryTensor, rel: &TernaryTensor, node_b: &TernaryTensor) -> i32 {
    let n = node_a.data.len().min(rel.data.len()).min(node_b.data.len());
    let mut sum = 0i32;
    for i in 0..n {
        sum += (node_a.data[i].to_i8() as i32)
            * (rel.data[i].to_i8() as i32)
            * (node_b.data[i].to_i8() as i32);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_propagate_two_node_chain() {
        // node 0 --edge--> node 1. node1's aggregated embedding should equal
        // node0's embedding (single neighbor, sum == that neighbor's values).
        let n0 = TernaryTensor::new(vec![Trit::Pos, Trit::Neg, Trit::Zero], vec![3]);
        let n1 = TernaryTensor::zeros(vec![3]);
        let out = propagate(&[n0.clone(), n1], &[(0, 1)]);
        assert_eq!(out[1].data, n0.data);
        // node 0 has no incoming edges -> stays zero
        assert_eq!(out[0].data, vec![Trit::Zero, Trit::Zero, Trit::Zero]);
    }

    #[test]
    fn test_propagate_saturation() {
        // two Pos-valued neighbors sum to +2 on that dim -> still saturates to Pos, not overflow
        let a = TernaryTensor::new(vec![Trit::Pos], vec![1]);
        let b = TernaryTensor::new(vec![Trit::Pos], vec![1]);
        let c = TernaryTensor::zeros(vec![1]);
        let out = propagate(&[a, b, c], &[(0, 2), (1, 2)]);
        assert_eq!(out[2].data, vec![Trit::Pos]);
    }

    #[test]
    fn test_distmult_score_matches_true_relation() {
        let node_a = TernaryTensor::new(vec![Trit::Pos, Trit::Pos], vec![2]);
        let node_b = TernaryTensor::new(vec![Trit::Pos, Trit::Neg], vec![2]);
        let true_rel = TernaryTensor::new(vec![Trit::Pos, Trit::Neg], vec![2]);
        // Pos*Pos*Pos=1, Pos*Neg*Neg=1 -> sum=2
        assert_eq!(distmult_score(&node_a, &true_rel, &node_b), 2);
        let wrong_rel = TernaryTensor::new(vec![Trit::Zero, Trit::Zero], vec![2]);
        assert_eq!(distmult_score(&node_a, &wrong_rel, &node_b), 0);
    }
}
