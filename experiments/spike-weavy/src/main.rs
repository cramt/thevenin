//! spike-weavy — model thevenin's `.control` interpreter on weavy's lowered-program
//! substrate.
//!
//! The `.control` language (thevenin-control) is run/let/print/alter/resume/quit.
//! weavy is a "shared lowered-program substrate": you bring your own `Op`
//! vocabulary + value model, lower a script into a `Lowered { program, blocks }`,
//! and weavy's stepper walks it with a real call stack. `resume` maps onto
//! weavy's async suspend lane (weavy::r#async) — this spike proves the
//! synchronous core (let/print + block calls / call frames); the suspend lane is
//! the next slice.

use std::collections::BTreeMap;
use std::collections::HashMap;

use weavy::{Control, Lowered, Step, run};

/// One `.control` instruction. Caller-defined — weavy is vocabulary-agnostic.
#[derive(Clone, Debug)]
enum Op {
    /// `let name = value`
    Let(String, f64),
    /// `print name`
    Print(String),
    /// invoke a named block (e.g. a `.control ... .endc` sub-block)
    Call(String),
}

/// Block ids are just names here; weavy only requires `Clone + Ord`.
type BlockId = String;

/// The interpreter IS the value model: env + captured output.
#[derive(Default)]
struct Control0 {
    env: HashMap<String, f64>,
    output: Vec<String>,
}

impl<'p> Step<'p, BlockId, Op> for Control0 {
    type Error = String;
    type Continuation = ();

    fn step(&mut self, op: &'p Op) -> Result<Control<'p, BlockId, Op, ()>, String> {
        match op {
            Op::Let(name, value) => {
                self.env.insert(name.clone(), *value);
                Ok(Control::Continue)
            }
            Op::Print(name) => {
                let v = self
                    .env
                    .get(name)
                    .ok_or_else(|| format!("print: undefined `{name}`"))?;
                self.output.push(format!("{name} = {v}"));
                Ok(Control::Continue)
            }
            // Hand control back to weavy's runner, which pushes a frame and
            // runs the named block, then resumes us at the next op.
            Op::Call(block) => Ok(Control::CallBlock(block.clone())),
        }
    }
}

fn main() {
    // Lowered form of:
    //   let vdd = 5
    //   call "measure"          ; a sub-block
    //   print out
    // .block measure:
    //   let out = 3.3
    let mut blocks: BTreeMap<BlockId, Vec<Op>> = BTreeMap::new();
    blocks.insert(
        "measure".to_string(),
        vec![Op::Let("out".to_string(), 3.3)],
    );

    let program = vec![
        Op::Let("vdd".to_string(), 5.0),
        Op::Call("measure".to_string()),
        Op::Print("out".to_string()),
        Op::Print("vdd".to_string()),
    ];

    let lowered = Lowered { program, blocks };

    let mut interp = Control0::default();
    match run(&lowered, &mut interp) {
        Ok(()) => {
            println!("[spike-weavy] ran {} ops via weavy stepper", interp.output.len());
            for line in &interp.output {
                println!("  {line}");
            }
            // Proves the call frame worked: `out` was set inside the block and
            // is visible after the block returned.
            assert_eq!(interp.env.get("out"), Some(&3.3));
            assert_eq!(interp.output, vec!["out = 3.3", "vdd = 5"]);
            println!("[spike-weavy] OK — call-frame + env survived block return");
        }
        Err(e) => {
            eprintln!("[spike-weavy] run error: {e:?}");
            std::process::exit(1);
        }
    }
}
