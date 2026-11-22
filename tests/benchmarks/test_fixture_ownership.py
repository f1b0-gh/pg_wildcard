# SPDX-License-Identifier: Apache-2.0
import json
import io
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import fixtures


class FixtureOwnership(unittest.TestCase):
    def test_elastic_resource_profiles_match_docker_and_private_state(self):
        for gib in (4, 8):
            with self.subTest(gib=gib), tempfile.TemporaryDirectory() as work:
                state=Path(work)/'elastic.json'
                class Opener:
                    def open(self, request, **kwargs):
                        return io.BytesIO(b'{"version":{"number":"test"}}')
                with patch('fixtures.command',side_effect=self.command) as command, patch('builtins.print'), patch('fixtures.urllib.request.build_opener',return_value=Opener()):
                    fixtures.start('elasticsearch',state,elastic_memory_gib=gib)
                data=json.loads(state.read_text())
                self.assertEqual(data['memory_bytes'],gib << 30)
                self.assertEqual(data['heap_bytes'],(gib//2) << 30)
                args=next(c.args for c in command.call_args_list if c.args[:2]==('docker','run'))
                self.assertEqual(args[args.index('--memory')+1],str(gib << 30))
                self.assertIn(f'ES_JAVA_OPTS=-Xms{gib//2}g -Xmx{gib//2}g',args)

    def test_invalid_resource_profile_creates_nothing(self):
        with tempfile.TemporaryDirectory() as work, patch('fixtures.command') as command:
            state=Path(work)/'elastic.json'
            with self.assertRaises(ValueError):
                fixtures.start('elasticsearch',state,elastic_memory_gib=16)
            command.assert_not_called()
            self.assertEqual(list(Path(work).iterdir()),[])

    def command(self, *args, **kwargs):
        if args[:2] == ('docker', 'inspect'):
            name=args[2]
            return json.dumps([{'NetworkSettings': {'Networks': {name+'-net': {'IPAddress':'127.0.0.1'}}}}])
        return 'test-version'

    def state(self, root, engine='elasticsearch'):
        path=root/'demo.json'
        path.write_text(json.dumps({'engine':engine,'name':'owned-container','network':'owned-network','owner':'test-owner'}))
        return path

    def test_start_preserves_parent_and_existing_sibling_artifacts(self):
        with tempfile.TemporaryDirectory() as work:
            root=Path(work);root.chmod(0o755)
            xml=root/'demo.xml';xml.write_text('belongs to caller')
            shared=root/'shared';shared.mkdir(mode=0o750)
            with patch('fixtures.command',side_effect=self.command), patch('builtins.print'):
                fixtures.start('antalya',root/'demo.json')
            self.assertEqual(stat.S_IMODE(root.stat().st_mode),0o755)
            self.assertEqual(xml.read_text(),'belongs to caller')
            self.assertEqual(stat.S_IMODE(shared.stat().st_mode),0o750)
            data=json.loads((root/'demo.json').read_text())
            artifacts=Path(data['artifact_dir'])
            self.assertEqual(artifacts.parent,root)
            self.assertEqual(stat.S_IMODE(artifacts.stat().st_mode),0o700)
            self.assertEqual(Path(data['share_dir']).parent,artifacts)
            self.assertEqual(stat.S_IMODE((root/'demo.json').stat().st_mode),0o600)
            def inspect(args,**kwargs):
                labels={'wildcard.benchmark.owner':data['owner']}
                info={'Config':{'Labels':labels}} if args[1]=='container' else {'Labels':labels}
                return subprocess.CompletedProcess(args,0,json.dumps([info]),'')
            with patch('fixtures.subprocess.run',side_effect=inspect), patch('fixtures.command'):
                fixtures.stop(root/'demo.json')
            self.assertFalse(artifacts.exists());self.assertFalse((root/'demo.json').exists())
            self.assertEqual(xml.read_text(),'belongs to caller')
            self.assertEqual(stat.S_IMODE(shared.stat().st_mode),0o750)

    def test_missing_image_does_not_chmod_caller_directory(self):
        with tempfile.TemporaryDirectory() as work:
            root=Path(work);root.chmod(0o755)
            with patch('fixtures.command',side_effect=subprocess.CalledProcessError(1,['docker'])):
                with self.assertRaises(subprocess.CalledProcessError):fixtures.start('antalya',root/'demo.json')
            self.assertEqual(stat.S_IMODE(root.stat().st_mode),0o755)
            self.assertEqual(list(root.iterdir()),[])

    def test_broken_state_symlink_is_not_followed(self):
        with tempfile.TemporaryDirectory() as work:
            root=Path(work);state=root/'demo.json';target=root/'unowned'
            state.symlink_to(target)
            with patch('fixtures.command',side_effect=self.command), patch('builtins.print'):
                with self.assertRaises((FileExistsError,RuntimeError)):fixtures.start('antalya',state)
            self.assertTrue(state.is_symlink());self.assertFalse(target.exists())

    def test_stop_missing_resources_preserves_unowned_sibling_xml(self):
        with tempfile.TemporaryDirectory() as work:
            root=Path(work);state=self.state(root)
            xml=root/'demo.xml';xml.write_text('unowned')
            def missing(args,**kwargs):
                return subprocess.CompletedProcess(args,1,'',f'Error response from daemon: No such {args[1]}: {args[3]}')
            with patch('fixtures.subprocess.run',side_effect=missing):fixtures.stop(state)
            self.assertFalse(state.exists());self.assertEqual(xml.read_text(),'unowned')

    def test_inspect_failure_retains_ownership_state_and_resources(self):
        with tempfile.TemporaryDirectory() as work:
            state=self.state(Path(work))
            failed=subprocess.CompletedProcess([],1,'','Cannot connect to the Docker daemon')
            with patch('fixtures.subprocess.run',return_value=failed), patch('fixtures.command') as command:
                with self.assertRaisesRegex(RuntimeError,'inspect'):fixtures.stop(state)
                command.assert_not_called()
            self.assertTrue(state.exists())


if __name__=='__main__':unittest.main()
