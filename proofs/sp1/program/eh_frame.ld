/* The C++ unwinder finds its tables by what registers them, and nothing does on a bare zkVM: this
   names where .eh_frame starts and ends it with the zero that marks its end, for main to register
   it, so a C++ exception Luau's parser raises unwinds to where Luau catches it. It adds to the
   linker's own layout and moves nothing else. After .rodata, which every program has: one that
   never reaches Luau, as the withdrawal program, has no .gcc_except_table to follow. */
SECTIONS
{
  .eh_frame : {
    __eh_frame_start = .;
    KEEP(*(.eh_frame))
    KEEP(*(.eh_frame.*))
    LONG(0);
  }
} INSERT AFTER .rodata;
