#!/usr/bin/python3

import rust_swig_test_python

print("rust_swig_test_python module imported.")
print("Doc: ", rust_swig_test_python.__doc__)
print("Members: ", dir(rust_swig_test_python))

print("TestEnum variants", dir(rust_swig_test_python.TestEnum))
rust_swig_test_python.TestStaticClass.print_hello()
rust_swig_test_python.TestStaticClass.print_number(123)
rust_swig_test_python.TestStaticClass.print_str("python str")
rust_swig_test_python.TestStaticClass.print_string("python string")
print(rust_swig_test_python.TestStaticClass.add(1, 2))

test_class = rust_swig_test_python.TestClass()
print(test_class)

test_class.print()
test_class.increment()
test_class.print()
test_class.add(3)
test_class.print()
print("test_class.get: ", test_class.get())

rust_swig_test_python.TestStaticClass.call_test_class_print(test_class)

enum = rust_swig_test_python.TestEnum.A
print("test_enum: ", enum)
print("test_enum: ", rust_swig_test_python.TestStaticClass.reverse_enum(enum))
