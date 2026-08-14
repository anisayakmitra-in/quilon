use super::*;
use crate::lexer::Lexer;
use crate::parser::parse;

#[test]
fn test_simple_number() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let tokens = Lexer::tokenize("x = 42").unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    // Global variable with float value
    assert!(ir.contains("4.2") || ir.contains("42"));
}

#[test]
fn test_simple_function() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let tokens = Lexer::tokenize("add = (a :: Num, b :: Num) -> Num => a + b").unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    assert!(result.is_ok());

    let ir = result.unwrap();
    assert!(ir.contains("define"));
    assert!(ir.contains("add"));
}

#[test]
fn test_local_variable() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    let code = "double = x :: Num => < y = x + x y >";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("alloca")); // Local variable
    assert!(ir.contains("load")); // Variable load
    assert!(ir.contains("store")); // Variable store
    assert!(ir.contains("fadd")); // Addition
}

#[test]
fn test_array() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    // Test array in a function body - return the first element as a number
    let code = "sum = x :: Num => < arr = [x, x, x] x >";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("alloca")); // Array allocation
    assert!(ir.contains("getelementptr")); // Array element access
}

#[test]
fn test_function_call() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    // Test calling a function
    let code = "
        add = (a :: Num, b :: Num) => a + b
        main = => add(3, 4)
    ";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("call")); // Function call
    assert!(ir.contains("@add")); // Call to add function
    assert!(ir.contains("fadd")); // Addition in add function
}

#[test]
fn test_record() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    // Test record creation
    let code = "make_point = (x :: Num, y :: Num) => < p = {x = x, y = y} x >";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("alloca")); // Struct allocation
    assert!(ir.contains("getelementptr")); // Field access
}

#[test]
fn test_field_access() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    // Test field access
    let code = "get_x = (a :: Num, b :: Num) => < p = {x = a, y = b} p.x >";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("getelementptr")); // Field GEP
    assert!(ir.contains("load")); // Field load
}

#[test]
fn test_method_codegen_and_dispatch() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    // A named record with a method; the entry point constructs an instance and calls it.
    // All fields are Num so the field layout/access is exact.
    let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y
}

^ = () -> Num => <
  p = Point { x = 3, y = 4 }
  p.sum()
>";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    // The method is emitted as a mangled top-level function taking the receiver pointer.
    assert!(ir.contains("@Point_sum"));
    // And the call site dispatches to it.
    assert!(ir.contains("call") && ir.contains("Point_sum"));
}

#[test]
fn test_method_calls_sibling_method() {
    let context = Context::create();
    let mut codegen = CodeGenerator::new(&context, "test");

    // `doubled` calls the sibling method `sum` via `it.sum()` — exercises the signature
    // pre-pass (forward reference) and `it`-based dispatch.
    let code = "Point = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y,
  doubled = => it.sum() + it.sum()
}

^ = () -> Num => <
  p = Point { x = 10, y = 5 }
  p.doubled()
>";
    let tokens = Lexer::tokenize(code).unwrap();
    let program = parse(&tokens).unwrap();

    let result = codegen.generate(&program);
    if let Err(e) = &result {
        println!("Error: {}", e);
    }
    assert!(result.is_ok());

    let ir = result.unwrap();
    println!("Generated IR:\n{}", ir);
    assert!(ir.contains("@Point_sum"));
    assert!(ir.contains("@Point_doubled"));
}
