# User model definition
from typing import Optional
from src.utils.database import db_manager

class User:
    def __init__(self, user_id: int, username: str, email: str):
        self.user_id = user_id
        self.username = username
        self.email = email

    @staticmethod
    def get_by_username(username: str) -> Optional['User']:
        """Retrieves a user by username from the database."""
        db_manager.connect()
        query = "SELECT * FROM users WHERE username = %s;"
        results = db_manager.execute_query(query, (username,))
        
        if results and results[0]:
            data = results[0]
            return User(user_id=data['id'], username=data['username'], email=data['email'])
        
        db_manager.close()
        return None

    @staticmethod
    def create_user(username: str, email: str) -> 'User':
        """Creates and saves a new user to the database."""
        db_manager.connect()
        query = "INSERT INTO users (username, email) VALUES (%s, %s);"
        db_manager.execute_query(query, (username, email))
        new_user = User(user_id=3, username=username, email=email) # Mocking ID assignment
        db_manager.close()
        return new_user