import os
import shutil
import tempfile
import types
import unittest
import importlib
from importlib.machinery import SourceFileLoader
from pathlib import Path


def load_howdone():
    """
    Load howdone module using importlib because it does not have the .py extension.
    """
    loader = SourceFileLoader("howdone", str(Path(__file__).parent / "howdone"))
    spec = importlib.util.spec_from_loader("howdone", loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


howdone_module = load_howdone()

# Assign the functions to test into this scope for easy access.
find_config_file = howdone_module.find_config_file
validate_output_dir_config = howdone_module.validate_output_dir_config
substitute_output_dir = howdone_module.substitute_output_dir
append_output_dir_arg = howdone_module.append_output_dir_arg
format_command_line = howdone_module.format_command_line
run = howdone_module.run


class TestFormatCommandLine(unittest.TestCase):
    def test_string_passthrough(self):
        self.assertEqual(
            format_command_line("echo hello"),
            "echo hello",
        )

    def test_list_joined(self):
        self.assertEqual(
            format_command_line(["echo", "hello", "world"]),
            "echo hello world",
        )

    def test_empty_list(self):
        self.assertEqual(
            format_command_line([]),
            "",
        )


class TestSubstituteOutputDir(unittest.TestCase):
    def test_substitutes_placeholder(self):
        self.assertEqual(
            substitute_output_dir("-o {OUTPUT_DIR}/out.txt", Path("/tmp/mydir")),
            "-o /tmp/mydir/out.txt",
        )

    def test_no_placeholder(self):
        self.assertEqual(
            substitute_output_dir("--flag", Path("/tmp/mydir")),
            "--flag",
        )

    def test_multiple_occurrences(self):
        self.assertEqual(
            substitute_output_dir("{OUTPUT_DIR} and {OUTPUT_DIR}", Path("/p")),
            "/p and /p",
        )


class TestAppendOutputDirArg(unittest.TestCase):
    def test_str_command_str_arg(self):
        self.assertEqual(
            append_output_dir_arg("prog", "-o {OUTPUT_DIR}", Path("/tmp/outdir")),
            "prog -o /tmp/outdir",
        )

    def test_str_command_list_arg(self):
        self.assertEqual(
            append_output_dir_arg("prog", ["-o", "{OUTPUT_DIR}"], Path("/tmp/outdir")),
            "prog -o /tmp/outdir",
        )

    def test_list_command_str_arg(self):
        self.assertEqual(
            append_output_dir_arg(["prog"], "-o {OUTPUT_DIR}", Path("/tmp/outdir")),
            ["prog", "-o", "/tmp/outdir"],
        )

    def test_list_command_list_arg(self):
        self.assertEqual(
            append_output_dir_arg(
                ["prog"], ["-o", "{OUTPUT_DIR}"], Path("/tmp/outdir")
            ),
            ["prog", "-o", "/tmp/outdir"],
        )


class TestValidateOutputDirConfig(unittest.TestCase):
    def test_no_output_dir_key(self):
        validate_output_dir_config({})  # must not raise

    def test_empty_output_dir(self):
        validate_output_dir_config({"output_dir": {}})  # must not raise

    def test_valid_variable(self):
        validate_output_dir_config({"output_dir": {"variable": "X"}})  # must not raise

    def test_valid_argument_string(self):
        validate_output_dir_config({"output_dir": {"argument": "-o {OUTPUT_DIR}"}})

    def test_valid_argument_list(self):
        validate_output_dir_config({"output_dir": {"argument": ["-o", "{OUTPUT_DIR}"]}})

    def test_argument_missing_placeholder_raises(self):
        with self.assertRaises(ValueError):
            validate_output_dir_config({"output_dir": {"argument": "-o /hardcoded"}})

    def test_argument_wrong_type_raises(self):
        with self.assertRaises(ValueError):
            validate_output_dir_config({"output_dir": {"argument": 42}})


class TestFindConfigFile(unittest.TestCase):
    def test_finds_in_start_dir(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            cfg = Path(tmpdir) / ".howdone.yaml"
            cfg.write_text("prefix: test\n")
            self.assertEqual(find_config_file(Path(tmpdir)), cfg)

    def test_finds_in_parent(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            cfg = Path(tmpdir) / ".howdone.yaml"
            cfg.write_text("prefix: test\n")
            child = Path(tmpdir) / "subdir"
            child.mkdir()
            self.assertEqual(find_config_file(child), cfg)

    def test_returns_none_when_not_found(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            child = Path(tmpdir) / "a" / "b"
            child.mkdir(parents=True)
            self.assertIsNone(find_config_file(child))


def make_args(**kwargs):
    defaults = dict(config=None, name=None, dir=None, prefix=None, cmd=["echo mainout"])
    defaults.update(kwargs)
    return types.SimpleNamespace(**defaults)


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self._orig_cwd = Path.cwd()
        self._tmpdir = Path(tempfile.mkdtemp())

    def tearDown(self):
        os.chdir(self._orig_cwd)
        shutil.rmtree(self._tmpdir, ignore_errors=True)

    def write_config(self, name, content):
        p = self._tmpdir / name
        p.write_text(content)
        return p

    def find_output_dir(self, prefix, base=None):
        base = base or self._tmpdir
        matches = list(Path(base).glob(f"{prefix}-*"))
        return matches[0] if matches else None

    def assertFileExists(self, path):
        self.assertTrue(path.is_file(), f"File does not exist: {path}")

    def assertDirExists(self, path):
        self.assertTrue(path.is_dir(), f"Directory does not exist: {path}")

    def assertFileContains(self, path, content):
        self.assertIn(content, path.read_text(), f"Content not found in file {path}")

    def test_autodiscover_config(self):
        """Auto-discover .howdone.yaml"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: autorun
output_file: output.txt
commands:
    side.txt: echo sideout
""",
        )
        os.chdir(self._tmpdir)
        run(make_args())
        self.outdir = self.find_output_dir("autorun")
        self.assertIsNotNone(self.outdir)
        self.assertDirExists(self.outdir)
        self.assertFileExists(self.outdir / "meta.yaml")
        self.assertFileExists(self.outdir / "output.txt")
        self.assertFileExists(self.outdir / "side.txt")
        self.assertFileContains(self.outdir / "output.txt", "mainout")

    def test_explicit_config(self):
        """Explicit config file via -c"""
        cfg = self.write_config(
            "my.yaml",
            """
prefix: explicitrun
output_file: output.txt
commands:
  side.txt: echo sideout
""",
        )
        workdir = self._tmpdir / "workdir"
        workdir.mkdir()
        os.chdir(workdir)
        run(make_args(config=cfg))
        self.outdir = self.find_output_dir("explicitrun", base=workdir)
        self.assertIsNotNone(self.outdir)
        self.assertFileExists(self.outdir / "side.txt")
        self.assertFileContains(self.outdir / "output.txt", "mainout")

    def test_prefix_from_config(self):
        """Output dir named with prefix from config"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: cfgprefix
output_file: output.txt
commands: {}
""",
        )
        os.chdir(self._tmpdir)
        run(make_args())
        outdir = self.find_output_dir("cfgprefix")
        self.assertIsNotNone(outdir)
        self.assertDirExists(outdir)

    def test_prefix_flag(self):
        """-p flag overrides config prefix"""
        super().setUp()
        self.write_config(
            ".howdone.yaml",
            """
prefix: cfgprefix
output_file: output.txt
commands: {}
""",
        )
        os.chdir(self._tmpdir)
        run(make_args(prefix="cmdprefix"))

        self.assertIsNotNone(self.find_output_dir("cmdprefix"))
        self.assertIsNone(self.find_output_dir("cfgprefix"))

    def test_exact_dir(self):
        """-d flag creates exact directory"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: someprefix
output_file: output.txt
commands: {}
""",
        )
        self.exact = self._tmpdir / "my_exact_dir"
        os.chdir(self._tmpdir)
        run(make_args(dir=self.exact))
        self.assertDirExists(self.exact)
        self.assertFileExists(self.exact / "output.txt")

    def test_env_var_received_by_main_command(self):
        """output_dir.variable passes env var to main command"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: vartest
output_file: output.txt
output_dir:
  variable: MY_OUTPUT_DIR
commands: {}
""",
        )
        os.chdir(self._tmpdir)
        run(make_args(cmd=["echo $MY_OUTPUT_DIR"]))
        outdir = self.find_output_dir("vartest")
        self.assertFileContains(outdir / "output.txt", str(outdir))

    def test_env_var_received_by_side_command(self):
        """output_dir.variable also passed to side commands"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: varside
output_file: output.txt
output_dir:
  variable: MY_OUTPUT_DIR
commands:
  sideenv.txt: echo $MY_OUTPUT_DIR
""",
        )
        os.chdir(self._tmpdir)
        run(make_args())
        outdir = self.find_output_dir("varside")
        self.assertFileContains(outdir / "sideenv.txt", str(outdir))

    def test_side_commands_run_in_output_dir(self):
        """output_dir.change_directory runs commands in output dir"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: cdtest
output_file: output.txt
output_dir:
  change_directory: true
commands:
  workdir.txt: pwd
""",
        )
        os.chdir(self._tmpdir)
        run(make_args())
        outdir = self.find_output_dir("cdtest")
        self.assertFileContains(outdir / "workdir.txt", str(outdir))

    def test_output_dir_appended_as_arg(self):
        """output_dir.argument as string appends to main command"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: argtest
output_file: output.txt
output_dir:
  argument: "-o {OUTPUT_DIR}"
commands: {}
""",
        )
        os.chdir(self._tmpdir)
        run(make_args())
        outdir = self.find_output_dir("argtest")
        self.assertIsNotNone(outdir)
        self.assertFileContains(outdir / "output.txt", f"-o {outdir}")

    def test_output_dir_appended_as_list_arg(self):
        """t10: output_dir.argument as list appends to main command"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: argtest
output_file: output.txt
output_dir:
  argument: ["-o", "{OUTPUT_DIR}"]
commands: {}
""",
        )
        os.chdir(self._tmpdir)
        run(make_args(cmd=["echo"]))
        outdir = self.find_output_dir("argtest")
        self.assertIsNotNone(outdir)
        self.assertFileContains(outdir / "output.txt", f"-o {outdir}")

    def test_before_commands_run_before_main(self):
        """Test commands with when: before run before main command"""
        self.write_config(
            ".howdone.yaml",
            """
prefix: whentest
output_file: output.txt
commands:
  before.txt:
    command: cat file.txt
    when: before
  after.txt:
    command: cat file.txt
    when: after
""",
        )
        os.chdir(self._tmpdir)
        run(make_args(cmd=["echo AFTER > file.txt"]))
        outdir = self.find_output_dir("whentest")
        self.assertIsNotNone(outdir)
        self.assertFileContains(outdir / "before.txt", "No such file or directory")
        self.assertFileContains(outdir / "after.txt", "AFTER")


if __name__ == "__main__":
    unittest.main()
