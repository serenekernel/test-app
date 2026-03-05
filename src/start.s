.section .text
.global _start
_start:
    mov rdi, rsp
    call _entry