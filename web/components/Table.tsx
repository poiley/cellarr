'use client';

import styles from '@components/Table.module.css';

import * as React from 'react';

type TableProps = React.HTMLAttributes<HTMLElement> & {
  children?: React.ReactNode;
};

const Table = ({ children, ...rest }) => {
  // Wrap every table in a horizontal-scroll container so wide tables scroll
  // instead of clipping columns / wrapping cells (e.g. on narrow viewports or
  // long content). The scrollbar only appears when the table overflows.
  return (
    <div style={{ overflowX: 'auto', maxWidth: '100%' }}>
      <table className={styles.root} {...rest}>
        <tbody className={styles.body}>{children}</tbody>
      </table>
    </div>
  );
};

Table.displayName = 'Table';

export default Table;
