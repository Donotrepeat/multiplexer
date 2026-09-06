# Authentication utility module
import jwt
from datetime import datetime

SECRET_KEY = "your_secret_key"

def create_access_token(user_id: int) -> str:
    """Creates a JWT access token for a given user ID."""
    payload = {
        "user_id": user_id,
        "exp": datetime.utcnow() + jwt.timedelta(minutes=30)
    }
    return jwt.encode(payload, SECRET_KEY, algorithm="HS256")

def verify_token(token: str) -> dict | None:
    """Verifies the JWT token and returns the payload if valid."""
    try:
        payload = jwt.decode(token, SECRET_KEY, algorithms=["HS256"])
        return payload
    except jwt.ExpiredSignatureError:
        print("Token expired")
        return None
    except jwt.InvalidTokenError:
        print("Invalid token")
        return None