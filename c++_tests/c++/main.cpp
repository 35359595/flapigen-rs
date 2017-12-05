#include <cstdint>
#include <cstdio>
#include <cassert>
#include <cstdbool>
#include <functional>
#include <atomic>

#include <gtest/gtest.h>
#include "rust_interface/RustStrView.h"
#include "rust_interface/c_SomeObserver.h"
#include "rust_interface/c_Foo.h"
#include "rust_interface/Foo.hpp"

static std::atomic<uint32_t> c_simple_cb_counter{0};

static void c_delete_int(void *opaque)
{
	printf("clear\n");
	auto self = static_cast<int *>(opaque);
	ASSERT_EQ(17, *self);
	delete self;
}

static void c_simple_cb(int32_t a, char b, void *opaque)
{
	assert(opaque != nullptr);
	const int tag = *static_cast<int *>(opaque);
	ASSERT_EQ(17, tag);
	printf("!!! a %d, b: %d, tag %d\n", static_cast<int>(a), b, tag);
	++c_simple_cb_counter;
}

TEST(c_Foo, Simple)
{
	auto foo = Foo_new(1, "a");
	ASSERT_NE(foo, nullptr);

	EXPECT_EQ(3, Foo_f(foo, 1, 1));
	auto name = Foo_getName(foo);
	EXPECT_EQ(std::string("a"), std::string(name.data, name.len));

	Foo_set_field(foo, 5);
	EXPECT_EQ(7, Foo_f(foo, 1, 1));
	const C_SomeObserver obs = {
		new int(17),
		c_delete_int,
		c_simple_cb,
	};
	c_simple_cb_counter = 0;
	Foo_call_me(&obs);
	EXPECT_EQ(1, c_simple_cb_counter.load());
	Foo_delete(foo);
}

TEST(Foo, Simple)
{
	Foo foo(1, "b");
	EXPECT_EQ(3, foo.f(1, 1));
	RustStrView name = foo.getName();
	EXPECT_EQ(std::string("b"), std::string(name.data, name.len));
	foo.set_field(5);
	EXPECT_EQ(7, foo.f(1, 1));
	const C_SomeObserver obs = {
		new int(17),
		c_delete_int,
		c_simple_cb,
	};
	c_simple_cb_counter = 0;
	Foo::call_me(&obs);
	EXPECT_EQ(1, c_simple_cb_counter.load());

	EXPECT_NEAR(7.5, foo.one_and_half(), 1e-16);
	{
		Foo f2(17, "");
		EXPECT_EQ(19, f2.f(1, 1));
		auto name = f2.getName();
		EXPECT_EQ(std::string(""), std::string(name.data, name.len));
	}
}

int main(int argc, char *argv[])
{
    ::testing::InitGoogleTest(&argc, argv);
    ::testing::GTEST_FLAG(throw_on_failure) = true;
    return RUN_ALL_TESTS();
}
