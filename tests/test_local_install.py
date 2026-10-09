import importlib.util
import pathlib
import unittest
from unittest.mock import patch

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
    def test_provisioning_is_private_narrow_and_cleans_up(self):
        calls=[]
        locations=[]
        key='A'*40
        queries=0
        def run(args,**kwargs):
            nonlocal queries
            calls.append(args)
            if args[1]=='find-identity':
                queries+=1
                return '' if queries==1 else f'  1) {key} "rclaude Local Development"\n'
            if args[1]=='default-keychain': return f'"{__file__}"\n'
            if args[1] in ('req','rsa'):
                self.assertEqual(args[0],'/usr/bin/openssl')
                path=pathlib.Path(args[args.index('-out')+1])
                path.write_bytes(b'synthetic certificate or key')
                locations.append(path.parent)
                if '-keyout' in args:
                    pathlib.Path(args[args.index('-keyout')+1]).write_bytes(b'synthetic key')
            if args[1]=='import':
                path=pathlib.Path(args[2])
                self.assertEqual(path.stat().st_mode & 0o777,0o600)
                self.assertIn('-x',args)
                self.assertEqual(args[args.index('-T')+1],'/usr/bin/codesign')
                self.assertNotIn('-A',args)
            return ''
        with patch.object(module.shutil,'which',return_value='/untrusted/path/openssl'):
            self.assertEqual(module.ensure_identity(run),key)
        self.assertTrue(locations)
        self.assertTrue(all(not path.exists() for path in locations))

if __name__=='__main__': unittest.main()
