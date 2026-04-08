use burn::{
    backend::NdArray,
    module::Module,
    nn::{Linear, LinearConfig, Relu},
    tensor::{Tensor, backend::Backend},
};
use burn_store::{BurnpackStore, ModuleSnapshot};
use spin::Once;

use crate::prelude::*;

#[derive(Module, Debug)]
pub struct SimpleLinearModel<B: Backend> {
    // We use a Vec to hold the dynamic number of hidden layers
    model: Vec<Linear<B>>,
    activation: Relu,
}

impl<B: Backend> SimpleLinearModel<B> {
    pub fn new(
        input_history: usize,
        num_input_features: usize,
        hidden_layer_sizes: Vec<usize>,
        output_size: usize,
        device: &B::Device,
    ) -> Self {
        let mut layers = Vec::new();
        let mut prev_layer_size = input_history * num_input_features;

        // Initialize hidden layers
        for &hidden_size in hidden_layer_sizes.iter() {
            layers.push(LinearConfig::new(prev_layer_size, hidden_size).init(device));
            prev_layer_size = hidden_size;
        }

        // Initialize output layer
        layers.push(LinearConfig::new(prev_layer_size, output_size).init(device));

        Self {
            model: layers,
            activation: Relu::new(),
        }
    }

    pub fn forward(&self, x: Tensor<B, 2>) -> Tensor<B, 2> {
        let mut x = x;

        // Iterate through hidden layers and apply ReLU
        for (i, layer) in self.model.iter().enumerate() {
            x = layer.forward(x);
            // Final output layer (no activation, matching your Python script)
            if (i != self.model.len() - 1) {
                x = self.activation.forward(x);
            }
        }

        x
    }
}

pub type MlpModel = SimpleLinearModel<NdArray<f32>>;

pub(super) static HUGEPAGE_MODEL: Once<Arc<Mutex<MlpModel>>> = Once::new();

static MODEL_WEIGHTS: &[u8] =
    include_bytes!("../../tools/pytorch_to_burnpack/weights/mlp_weights.bpk");
pub fn init_mlp_model() {
    let device = Default::default();
    let mut model: SimpleLinearModel<NdArray<f32>> =
        SimpleLinearModel::new(5, 4, vec![64, 32], 1, &device);
    crate::prelude::println!("weights: {}", MODEL_WEIGHTS.len());
    let mut store = BurnpackStore::from_static(MODEL_WEIGHTS);
    model.load_from(&mut store).unwrap();
    HUGEPAGE_MODEL.call_once(|| Arc::new(Mutex::new(model)));
}

pub fn get_mlp_model() -> Arc<Mutex<MlpModel>> {
    HUGEPAGE_MODEL.wait().clone()
}
