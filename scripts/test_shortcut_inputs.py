"""Check the packaged variable bindings, which Swift unit tests do not import."""
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ShortcutInputTests(unittest.TestCase):
    def test_shared_text_is_a_string_containing_shortcut_input(self):
        workflow = json.loads((ROOT / 'ios/Shortcuts/Send to xlatch.json').read_text())
        action = workflow['WFWorkflowActions'][0]['WFWorkflowActionParameters']
        self.assertEqual(action['text'], {
            'WFSerializationType': 'WFTextTokenString',
            'Value': {'string': '\ufffc', 'attachmentsByRange': {
                '{0, 1}': {'Type': 'ExtensionInput'}
            }}
        })
        self.assertIn('ActionExtension', workflow['WFWorkflowTypes'])
        self.assertEqual(workflow['WFWorkflowImportQuestions'][0]['ParameterKey'], 'target')

    def test_string_parameters_do_not_use_file_attachment_encoding(self):
        for path in (ROOT / 'ios/Shortcuts').glob('*.json'):
            for action in json.loads(path.read_text())['WFWorkflowActions']:
                if not action['WFWorkflowActionIdentifier'].startswith('com.byteowlz.xlatch.'):
                    continue
                for name, value in action['WFWorkflowActionParameters'].items():
                    if name in {'text', 'sharedText', 'pageTitle', 'pageText', 'note'} and isinstance(value, dict):
                        with self.subTest(workflow=path.name, parameter=name):
                            self.assertEqual(value['WFSerializationType'], 'WFTextTokenString')


if __name__ == '__main__':
    unittest.main()
