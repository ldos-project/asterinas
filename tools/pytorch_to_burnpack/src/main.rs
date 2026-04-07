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

    let mut model: SimpleLinearModel<NdArray<f32>> =
        SimpleLinearModel::new(5, 4, vec![64, 32], 1, &device);
    println!("?!? loading model weights");
    let mut store = BurnpackStore::from_file("weights/mlp_weights.bpk");
    model
        .load_from(&mut store)
        .unwrap_or_else(|e| panic!("Failed to load bpk model weights: {e}"));
    println!("?!? constructing a tensor");

    /*
    0    208          9502672          7117333.0        -530376.0   5712
    1    199          9502672          9502672.0              0.0   5703
    2    198          9502672          9502672.0              0.0   5702
    3    193          9502672          9502672.0              0.0   5697
    4    188          9502672          9502672.0              0.0   5692
    */

    let l1_tlb_mean = 17962217092.36901;
    let l1_tlb_std = 10438282560.234997;
    let pgfault_std = 5907488.000634078;
    let pgfault_mean = -751.5422939879493;
    let rss_std = 225.91115460963047;
    let rss_mean = 5739.57650502839;
    let bloat_std = 243.9566731489499;
    let bloat_mean = 207.82560494187038;

    let l1_trans = |x| (x - l1_tlb_mean) / l1_tlb_std;
    let pgfault_trans = |x| (x - pgfault_mean) / pgfault_std;
    let rss_trans = |x| (x - rss_mean) / rss_std;

    let bloat_untrans = |x| x * bloat_std + bloat_mean;

    let t: Tensor<NdArray<f32>, 2> = Tensor::from_floats(
        [
            [
                l1_trans(9502672.0),
                l1_trans(7117333.0),
                pgfault_trans(-530376.0),
                rss_trans(5712.0),
            ],
            [
                l1_trans(9502672.0),
                l1_trans(9502672.0),
                pgfault_trans(0.0),
                rss_trans(5703.0),
            ],
            [
                l1_trans(9502672.0),
                l1_trans(9502672.0),
                pgfault_trans(0.0),
                rss_trans(5702.0),
            ],
            [
                l1_trans(9502672.0),
                l1_trans(9502672.0),
                pgfault_trans(0.0),
                rss_trans(5697.0),
            ],
            [
                l1_trans(9502672.0),
                l1_trans(9502672.0),
                pgfault_trans(0.0),
                rss_trans(5692.0),
            ],
        ],
        &device,
    );
    // Flatten the array into 1x?
    let t = t.reshape([1, -1]);
    println!("?!? input={:?}", t);
    let t = model.forward(t);
    println!("?!? output={:?}", bloat_untrans(t.into_scalar()));
}
