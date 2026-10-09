import importlib.util
import pathlib
import unittest

ROOT=pathlib.Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('local_install',ROOT/'scripts/install_local_macos.py')
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class LocalSigningTests(unittest.TestCase):
    def test_only_the_exact_local_identity_is_reused(self):
        key='A'*40
        self.assertEqual(module.find_identity(f'  1) {key} "rclaude Local Development"\n'),key)
        self.assertIsNone(module.find_identity(f'  1) {key} "Unrelated signer"\n'))
        self.assertIsNone(module.find_identity('  1) bad "rclaude Local Development"\n'))
    def test_requirement_pins_certificate_and_identifier(self):
        key='A'*40
        requirement=module.requirement(key)
        self.assertTrue(requirement.startswith("=designated =>"))
        self.assertIn('certificate leaf = H"'+key+'"',requirement)
        self.assertIn('identifier "io.github.typiqally.rclaude"',requirement)
        with self.assertRaises(ValueError): module.requirement('" or true')
    def test_an_existing_identity_prevents_new_key_creation(self):
        calls=[]
        key='A'*40
        def run(args,**kwargs):
            calls.append(args)
            return f'  1) {key} "rclaude Local Development"\n'
        self.assertEqual(module.ensure_identity(run),key)
        self.assertEqual(len(calls),1)

if __name__=='__main__': unittest.main()
