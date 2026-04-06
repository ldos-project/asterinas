use std::{env, path::Path, process};

use burn::{
    backend::NdArray,
    nn::{
        BatchNorm, BatchNormConfig, Linear, LinearConfig,
        conv::{Conv2d, Conv2dConfig},
    },
    prelude::*,
    tensor::activation::{log_softmax, relu},
};
use burn_store::{
    BurnpackStore, ModuleSnapshot, PyTorchToBurnAdapter, PytorchStore, SafetensorsStore,
};

#[derive(Module, Debug)]
pub struct Model<B: Backend> {
    conv1: Conv2d<B>,
    conv2: Conv2d<B>,
    conv3: Conv2d<B>,
    norm1: BatchNorm<B>,
    fc1: Linear<B>,
    fc2: Linear<B>,
    norm2: BatchNorm<B>,
}

impl<B: Backend> Model<B> {
    pub fn init(device: &B::Device) -> Self {
        let conv1 = Conv2dConfig::new([1, 8], [3, 3]).init(device);
        let conv2 = Conv2dConfig::new([8, 16], [3, 3]).init(device);
        let conv3 = Conv2dConfig::new([16, 24], [3, 3]).init(device);
        let norm1 = BatchNormConfig::new(24).init(device);
        let fc1 = LinearConfig::new(11616, 32).init(device);
        let fc2 = LinearConfig::new(32, 10).init(device);
        let norm2 = BatchNormConfig::new(10).init(device);

        Self {
            conv1,
            conv2,
            conv3,
            norm1,
            fc1,
            fc2,
            norm2,
        }
    }

    pub fn forward(&self, input1: Tensor<B, 4>) -> Tensor<B, 2> {
        let conv1_out1 = self.conv1.forward(input1);
        let relu1_out1 = relu(conv1_out1);
        let conv2_out1 = self.conv2.forward(relu1_out1);
        let relu2_out1 = relu(conv2_out1);
        let conv3_out1 = self.conv3.forward(relu2_out1);
        let relu3_out1 = relu(conv3_out1);
        let norm1_out1 = self.norm1.forward(relu3_out1);
        let flatten1_out1 = norm1_out1.flatten(1, 3);
        let fc1_out1 = self.fc1.forward(flatten1_out1);
        let relu4_out1 = relu(fc1_out1);
        let fc2_out1 = self.fc2.forward(relu4_out1);
        let norm2_out1 = self.norm2.forward(fc2_out1);
        log_softmax(norm2_out1, 1)
    }
}

use burn_store::{BurnpackStore, ModuleSnapshot};

// Path constants
const PYTORCH_WEIGHTS_PATH: &str = "weights/model.pt";
const MODEL_OUTPUT_NAME: &str = "model";

// Basic backend type (not used for computation).
type B = NdArray<f32>;

pub fn main() {
    let args: Vec<String> = env::args().collect();

    // Check argument count
    if args.len() < 3 {
        eprintln!("Usage: {} <output_directory>", args[0]);
        process::exit(1);
    }

    // Get weight format and output directory from arguments
    let output_directory = Path::new(&args[1]);

    // Use the default device (CPU)
    let device = Default::default();

    // Initialize a model with default weights
    let mut model: Model<B> = Model::init(&device);

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
