# Test file for authentication utilities
import unittest
from src.utils.auth import create_access_token, verify_token

class TestAuth(unittest.TestCase):
    def test_create_access_token(self):
        # Test token creation and structure
        user_id = 101
        token = create_access_token(user_id)
        self.assertIsInstance(token, str)
        print(f"Generated Token: {token}")

    def test_verify_token_valid(self):
        # Test with a valid token (assuming token generation works)
        user_id = 102
        valid_token = create_access_token(user_id)
        payload = verify_token(valid_token)
        self.assertIsNotNone(payload)
        self.assertEqual(payload.get('user_id'), user_id)

    def test_verify_token_invalid(self):
        # Test with an intentionally invalid token
        invalid_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.invalid."
        payload = verify_token(invalid_token)
        self.assertIsNone(payload)

if __name__ == '__main__':
    unittest.main()