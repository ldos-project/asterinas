use std::{env, path::Path, process};

use burn::{
    backend::NdArray,
    nn::{
        BatchNorm, BatchNormConfig, Linear, LinearConfig, Relu,
        conv::{Conv2d, Conv2dConfig},
    },
    prelude::*,
    tensor::activation::{log_softmax, relu},
};
use burn_store::{
    BurnpackStore, ModuleSnapshot, PyTorchToBurnAdapter, PytorchStore, SafetensorsStore,
};

#[derive(Module, Debug)]
pub struct SimpleLinearModel<B: Backend> {
    // We use a Vec to hold the dynamic number of hidden layers
    hidden_layers: Vec<Linear<B>>,
    output_layer: Linear<B>,
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
        let output_layer = LinearConfig::new(prev_layer_size, output_size).init(device);

        Self {
            hidden_layers: layers,
            output_layer,
            activation: Relu::new(),
        }
    }

    pub fn forward(&self, x: Tensor<B, 2>) -> Tensor<B, 2> {
        let mut x = x;

        // Iterate through hidden layers and apply ReLU
        for layer in self.hidden_layers.iter() {
            x = layer.forward(x);
            x = self.activation.forward(x);
        }

        // Final output layer (no activation, matching your Python script)
        self.output_layer.forward(x)
    }
}

// Path constants
const PYTORCH_WEIGHTS_PATH: &str = "weights/mlp_weights.pt";
const MODEL_OUTPUT_NAME: &str = "mlp_weights";

// Basic backend type (not used for computation).
type B = NdArray<f32>;

pub fn main() {
    let args: Vec<String> = env::args().collect();

    // Check argument count
    if args.len() < 2 {
        eprintln!("Usage: {} <output_directory>", args[0]);
        process::exit(1);
    }

    // Get weight format and output directory from arguments
    let output_directory = Path::new(&args[1]);

    // Use the default device (CPU)
    let device = Default::default();

    // Initialize a model with default weights
    let mut model: SimpleLinearModel<B> = SimpleLinearModel::new(5, 4, vec![64, 32], 1, &device);

    // Load the model weights based on the specified format
    println!("Loading PyTorch weights from '{PYTORCH_WEIGHTS_PATH}'...");
    let mut store = PytorchStore::from_file(PYTORCH_WEIGHTS_PATH);
    model.load_from(&mut store).unwrap_or_else(|e| {
        panic!("Failed to load PyTorch model weights from '{PYTORCH_WEIGHTS_PATH}': {e}")
    });

    // Define the output path for the Burn model file
    let output_file_path = output_directory.join(MODEL_OUTPUT_NAME);
    println!("Saving model to '{}.bpk'...", output_file_path.display());

    // Save the model using BurnpackStore
    let mut store = BurnpackStore::from_file(&output_file_path).overwrite(true);
    model.save_into(&mut store).unwrap_or_else(|e| {
        panic!(
            "Failed to save model to '{}.bpk': {e}",
            output_file_path.display()
        )
    });

    println!(
        "Model successfully saved to '{}.bpk'.",
        output_file_path.display()
    );
}
