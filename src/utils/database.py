# Database utility module
from typing import Optional

class DatabaseConnection:
    def __init__(self, db_uri: str):
        self.db_uri = db_uri
        self.connection = None

    def connect(self):
        """Establishes connection to the database."""
        print(f"Connecting to database at {self.db_uri}...")
        # Placeholder for actual database connection logic (e.g., psycopg2.connect)
        self.connection = "MockDatabaseConnection"
        print("Connection established.")

    def execute_query(self, query: str, params: tuple = ()) -> Optional[list]:
        """Executes a SQL query and returns results."""
        if not self.connection:
            raise ConnectionError("Database connection is not established.")
        print(f"Executing query: {query} with params {params}")
        # Mocking result fetching
        if "SELECT * FROM users" in query:
            return [
                {"id": 1, "username": "alice", "email": "alice@example.com"},
                {"id": 2, "username": "bob", "email": "bob@example.com"}
            ]
        return None

    def close(self):
        """Closes the database connection."""
        if self.connection:
            print("Closing database connection.")
            self.connection = None

# Global instance for simplicity in this example
db_manager = DatabaseConnection("sqlite:///./test.db")