#ifndef BINDER_TRACE_KMOD_SYMBOLS_H
#define BINDER_TRACE_KMOD_SYMBOLS_H

struct bt_binder_symbols {
    unsigned long binder_ioctl;
    unsigned long binder_alloc_copy_user_to_buffer;
    unsigned long binder_transaction;
};

extern struct bt_binder_symbols bt_binder_symbols;

int bt_binder_symbols_init(void);

#endif /* BINDER_TRACE_KMOD_SYMBOLS_H */
