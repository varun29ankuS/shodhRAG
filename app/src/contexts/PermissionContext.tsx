import React, { createContext, useContext, useState, useCallback } from 'react';
import { PermissionDialog, PermissionRequest } from '../components/PermissionDialog';

interface PermissionContextType {
  requestPermission: (request: PermissionRequest) => Promise<{ allowed: boolean; scope: 'once' | 'session' }>;
}

const PermissionContext = createContext<PermissionContextType | undefined>(undefined);

export function usePermission() {
  const context = useContext(PermissionContext);
  if (!context) {
    throw new Error('usePermission must be used within a PermissionProvider');
  }
  return context;
}

interface PendingRequest {
  request: PermissionRequest;
  resolve: (decision: { allowed: boolean; scope: 'once' | 'session' }) => void;
  reject: (error: Error) => void;
}

export function PermissionProvider({ children }: { children: React.ReactNode }) {
  const [currentRequest, setCurrentRequest] = useState<PendingRequest | null>(null);
  const [requestQueue, setRequestQueue] = useState<PendingRequest[]>([]);

  const processNextRequest = useCallback(() => {
    setRequestQueue((queue) => {
      if (queue.length === 0) {
        setCurrentRequest(null);
        return queue;
      }
      const [next, ...rest] = queue;
      setCurrentRequest(next);
      return rest;
    });
  }, []);

  const requestPermission = useCallback(
    (request: PermissionRequest): Promise<{ allowed: boolean; scope: 'once' | 'session' }> => {
      return new Promise((resolve, reject) => {
        const pendingRequest: PendingRequest = {
          request,
          resolve,
          reject,
        };

        // If no request is currently being shown, show this one immediately
        if (!currentRequest) {
          setCurrentRequest(pendingRequest);
        } else {
          // Otherwise, add to queue
          setRequestQueue((queue) => [...queue, pendingRequest]);
        }
      });
    },
    [currentRequest]
  );

  const handleDecision = useCallback(
    (allowed: boolean, scope: 'once' | 'session') => {
      if (currentRequest) {
        currentRequest.resolve({ allowed, scope });
        processNextRequest();
      }
    },
    [currentRequest, processNextRequest]
  );

  const handleClose = useCallback(() => {
    if (currentRequest) {
      // Closing without decision = deny
      currentRequest.resolve({ allowed: false, scope: 'once' });
      processNextRequest();
    }
  }, [currentRequest, processNextRequest]);

  return (
    <PermissionContext.Provider value={{ requestPermission }}>
      {children}
      <PermissionDialog
        isOpen={currentRequest !== null}
        request={currentRequest?.request || null}
        onDecision={handleDecision}
        onClose={handleClose}
      />
    </PermissionContext.Provider>
  );
}
