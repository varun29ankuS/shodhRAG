import React, { createContext, useContext, useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { logger } from '../utils/logger';

export enum UserRole {
  SUPER_ADMIN = 'super_admin',
  ADMIN = 'admin',
  MANAGER = 'manager',
  USER = 'user',
  VIEWER = 'viewer'
}

export interface User {
  id: string;
  username: string;
  email: string;
  role: UserRole;
  department?: string;
  permissions: string[];
  lastLogin: string;
  isActive: boolean;
  metadata?: Record<string, unknown>;
}

export interface AuthState {
  isAuthenticated: boolean;
  user: User | null;
  loading: boolean;
  error: string | null;
}

interface AuthContextType extends AuthState {
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  checkPermission: (permission: string) => boolean;
  hasRole: (role: UserRole | UserRole[]) => boolean;
  refreshToken: () => Promise<void>;
  updateProfile: (updates: Partial<User>) => Promise<void>;
}

const AuthContext = createContext<AuthContextType | null>(null);

const DEFAULT_PERMISSIONS: Record<UserRole, string[]> = {
  [UserRole.SUPER_ADMIN]: ['*'],
  [UserRole.ADMIN]: [
    'users.view', 'users.create', 'users.edit', 'users.delete',
    'spaces.view', 'spaces.create', 'spaces.edit', 'spaces.delete',
    'documents.view', 'documents.create', 'documents.edit', 'documents.delete',
    'analytics.view', 'settings.view', 'settings.edit',
    'backup.view', 'backup.create', 'backup.restore'
  ],
  [UserRole.MANAGER]: [
    'users.view', 'users.edit',
    'spaces.view', 'spaces.create', 'spaces.edit',
    'documents.view', 'documents.create', 'documents.edit', 'documents.delete',
    'analytics.view'
  ],
  [UserRole.USER]: [
    'spaces.view', 'spaces.create',
    'documents.view', 'documents.create', 'documents.edit',
    'analytics.view.own'
  ],
  [UserRole.VIEWER]: [
    'spaces.view',
    'documents.view'
  ]
};

export const AuthProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [state, setState] = useState<AuthState>({
    isAuthenticated: false,
    user: null,
    loading: true,
    error: null
  });

  useEffect(() => {
    checkAuthStatus();
  }, []);

  const checkAuthStatus = async () => {
    try {
      const token = localStorage.getItem('auth_token');
      if (!token) {
        setState(prev => ({ ...prev, loading: false }));
        return;
      }

      const user = await invoke<User>('verify_token', { token });
      setState({
        isAuthenticated: true,
        user,
        loading: false,
        error: null
      });
    } catch (error) {
      logger.error('Auth check failed', { error });
      localStorage.removeItem('auth_token');
      setState({
        isAuthenticated: false,
        user: null,
        loading: false,
        error: null
      });
    }
  };

  const login = async (username: string, password: string) => {
    setState(prev => ({ ...prev, loading: true, error: null }));
    
    try {
      const response = await invoke<{ user: User; token: string }>('authenticate', {
        username,
        password
      });

      localStorage.setItem('auth_token', response.token);
      
      setState({
        isAuthenticated: true,
        user: response.user,
        loading: false,
        error: null
      });

      logger.info('User logged in', { userId: response.user.id, role: response.user.role });
    } catch (error: any) {
      const message = error.message || 'Authentication failed';
      setState(prev => ({
        ...prev,
        loading: false,
        error: message
      }));
      logger.error('Login failed', { error, username });
      throw new Error(message);
    }
  };

  const logout = async () => {
    try {
      const token = localStorage.getItem('auth_token');
      if (token) {
        await invoke('logout', { token });
      }
    } catch (error) {
      logger.error('Logout error', { error });
    } finally {
      localStorage.removeItem('auth_token');
      setState({
        isAuthenticated: false,
        user: null,
        loading: false,
        error: null
      });
      logger.info('User logged out');
    }
  };

  const checkPermission = (permission: string): boolean => {
    if (!state.user) return false;
    
    // Super admin has all permissions
    if (state.user.role === UserRole.SUPER_ADMIN) return true;
    
    // Check wildcard permissions
    if (state.user.permissions.includes('*')) return true;
    
    // Check specific permission
    if (state.user.permissions.includes(permission)) return true;
    
    // Check partial wildcard (e.g., 'users.*' matches 'users.view')
    const permissionParts = permission.split('.');
    for (let i = permissionParts.length; i > 0; i--) {
      const wildcardPerm = permissionParts.slice(0, i - 1).join('.') + '.*';
      if (state.user.permissions.includes(wildcardPerm)) return true;
    }
    
    return false;
  };

  const hasRole = (role: UserRole | UserRole[]): boolean => {
    if (!state.user) return false;
    
    const roles = Array.isArray(role) ? role : [role];
    return roles.includes(state.user.role);
  };

  const refreshToken = async () => {
    try {
      const token = localStorage.getItem('auth_token');
      if (!token) throw new Error('No token available');

      const response = await invoke<{ token: string }>('refresh_token', { token });
      localStorage.setItem('auth_token', response.token);
      
      logger.info('Token refreshed');
    } catch (error) {
      logger.error('Token refresh failed', { error });
      await logout();
      throw error;
    }
  };

  const updateProfile = async (updates: Partial<User>) => {
    if (!state.user) throw new Error('Not authenticated');
    
    try {
      const updatedUser = await invoke<User>('update_profile', {
        userId: state.user.id,
        updates
      });
      
      setState(prev => ({
        ...prev,
        user: updatedUser
      }));
      
      logger.info('Profile updated', { userId: state.user.id });
    } catch (error) {
      logger.error('Profile update failed', { error });
      throw error;
    }
  };

  return (
    <AuthContext.Provider value={{
      ...state,
      login,
      logout,
      checkPermission,
      hasRole,
      refreshToken,
      updateProfile
    }}>
      {children}
    </AuthContext.Provider>
  );
};

export const useAuth = () => {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within AuthProvider');
  }
  return context;
};

// Permission helper hooks
export const usePermission = (permission: string): boolean => {
  const { checkPermission } = useAuth();
  return checkPermission(permission);
};

export const useRole = (role: UserRole | UserRole[]): boolean => {
  const { hasRole } = useAuth();
  return hasRole(role);
};

// Protected route component
export const ProtectedRoute: React.FC<{
  children: React.ReactNode;
  permission?: string;
  role?: UserRole | UserRole[];
  fallback?: React.ReactNode;
}> = ({ children, permission, role, fallback }) => {
  const { isAuthenticated, loading, checkPermission, hasRole } = useAuth();
  
  if (loading) {
    return <div>Loading...</div>;
  }
  
  if (!isAuthenticated) {
    return <>{fallback || <div>Please log in to continue</div>}</>;
  }
  
  if (permission && !checkPermission(permission)) {
    return <>{fallback || <div>You don't have permission to access this resource</div>}</>;
  }
  
  if (role && !hasRole(role)) {
    return <>{fallback || <div>Your role doesn't have access to this resource</div>}</>;
  }
  
  return <>{children}</>;
};

export default AuthContext;