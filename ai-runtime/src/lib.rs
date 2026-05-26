use ternary_core::Trit;

#[derive(Debug, Clone)]
pub struct TernaryTensor {
    pub data: Vec<Trit>,
    pub shape: Vec<usize>,
}

impl TernaryTensor {
    pub fn new(data: Vec<Trit>, shape: Vec<usize>) -> Self {
        let size: usize = shape.iter().product();
        assert_eq!(data.len(), size, "Data size must match shape product");
        Self { data, shape }
    }

    pub fn zeros(shape: Vec<usize>) -> Self {
        let size: usize = shape.iter().product();
        Self {
            data: vec![Trit::Zero; size],
            shape,
        }
    }

    /// Optimized dot product with "Activation Skipping".
    /// Returns (Sum, Total Ops, Skipped Ops)
    pub fn sparse_dot(a: &[Trit], b: &[Trit]) -> (i32, usize, usize) {
        assert_eq!(a.len(), b.len());
        let mut sum = 0;
        let mut skipped = 0;
        let total = a.len();

        for i in 0..total {
            match (a[i], b[i]) {
                (Trit::Zero, _) | (_, Trit::Zero) => {
                    skipped += 1;
                    // Skip computation
                }
                (Trit::Pos, Trit::Pos) | (Trit::Neg, Trit::Neg) => {
                    sum += 1;
                }
                (Trit::Pos, Trit::Neg) | (Trit::Neg, Trit::Pos) => {
                    sum -= 1;
                }
            }
        }

        (sum, total, skipped)
    }
}

pub struct TernaryLinear {
    pub weights: TernaryTensor, // Shape: [out_features, in_features]
}

impl TernaryLinear {
    pub fn new(in_features: usize, out_features: usize) -> Self {
        // Default to zeros for now
        Self {
            weights: TernaryTensor::zeros(vec![out_features, in_features]),
        }
    }

    pub fn forward(&self, input: &TernaryTensor) -> (TernaryTensor, usize, usize) {
        assert_eq!(input.shape.len(), 1, "Input must be a vector for now");
        let in_features = self.weights.shape[1];
        let out_features = self.weights.shape[0];
        assert_eq!(input.data.len(), in_features);

        let mut output_data = Vec::with_capacity(out_features);
        let mut total_ops = 0;
        let mut total_skipped = 0;

        for row in 0..out_features {
            let start = row * in_features;
            let end = start + in_features;
            let weight_row = &self.weights.data[start..end];
            
            let (sum, ops, skipped) = TernaryTensor::sparse_dot(&input.data, weight_row);
            
            // Native Ternary Activation: Step Function
            let activation = if sum > 0 {
                Trit::Pos
            } else if sum < 0 {
                Trit::Neg
            } else {
                Trit::Zero
            };
            
            output_data.push(activation);
            total_ops += ops;
            total_skipped += skipped;
        }

        (TernaryTensor::new(output_data, vec![out_features]), total_ops, total_skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sparse_dot() {
        let a = vec![Trit::Pos, Trit::Zero, Trit::Neg];
        let b = vec![Trit::Pos, Trit::Pos, Trit::Pos];
        let (sum, total, skipped) = TernaryTensor::sparse_dot(&a, &b);
        assert_eq!(sum, 0); // (1*1) + (0*1) + (-1*1) = 0
        assert_eq!(total, 3);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn test_linear_layer() {
        let mut layer = TernaryLinear::new(2, 1);
        layer.weights.data = vec![Trit::Pos, Trit::Neg];
        
        let input = TernaryTensor::new(vec![Trit::Pos, Trit::Zero], vec![2]);
        let (output, _total, skipped) = layer.forward(&input);
        
        // (1*1) + (0*-1) = 1 => Pos
        assert_eq!(output.data[0], Trit::Pos);
        assert_eq!(skipped, 1);
    }
}
