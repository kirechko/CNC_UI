"""Protocol unit tests that run without LinuxCNC or an NML process."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from protocol import failure, parse_packet, success


class ProtocolTests(unittest.TestCase):
    """Verify request validation and stable JSON response envelopes."""

    def test_rejects_unsupported_versions_and_commands(self):
        with self.assertRaises(ValueError):
            parse_packet(b'{"v":2,"id":1,"command":"status","payload":{}}')
        with self.assertRaises(ValueError):
            parse_packet(b'{"v":1,"id":1,"command":"rm -rf","payload":{}}')
        dynamic_mode = parse_packet(b'{"v":1,"id":3,"command":"set_laser_mode","payload":{"mode":"dynamic"}}')
        self.assertEqual(dynamic_mode["command"], "set_laser_mode")

    def test_validates_request_shape(self):
        packet = parse_packet(b'{"v":1,"id":7,"command":"mdi","payload":{"block":"G0 X0"}}')
        self.assertEqual(packet["payload"]["block"], "G0 X0")
        with self.assertRaises(ValueError):
            parse_packet(b'{"v":1,"id":-1,"command":"status","payload":{}}')
        with self.assertRaises(ValueError):
            parse_packet(b'{"v":1,"id":1,"command":"status","payload":[]}')

    def test_response_envelopes_echo_ids(self):
        """Response helpers retain ids and encode failures without exception objects."""
        self.assertEqual(success(12, {"alive": True}), {"v": 1, "id": 12, "ok": True, "payload": {"alive": True}})
        self.assertEqual(failure(12, "timeout", "late" )["error"]["code"], "timeout")


if __name__ == "__main__":
    unittest.main()
