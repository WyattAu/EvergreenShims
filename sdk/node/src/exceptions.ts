export class EvergreenShimError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "EvergreenShimError";
  }
}

export class APIError extends EvergreenShimError {
  public statusCode: number;
  public body: string;

  constructor(statusCode: number, body: string) {
    super(`API error ${statusCode}: ${body}`);
    this.name = "APIError";
    this.statusCode = statusCode;
    this.body = body;
  }
}

export class ConnectionError extends EvergreenShimError {
  constructor(message: string) {
    super(message);
    this.name = "ConnectionError";
  }
}

export class TimeoutError extends EvergreenShimError {
  constructor(message: string) {
    super(message);
    this.name = "TimeoutError";
  }
}
