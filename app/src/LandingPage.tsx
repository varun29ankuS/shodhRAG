import React, { useState } from 'react';
import './LandingPage.css';

interface LandingPageProps {
  onGetStarted: () => void;
  onTryDemo: () => void;
}

const LandingPage: React.FC<LandingPageProps> = ({ onGetStarted, onTryDemo }) => {
  const [activeTab, setActiveTab] = useState('features');

  const features = [
    {
      icon: "🔒",
      title: "100% Local",
      description: "Your data never leaves your device"
    },
    {
      icon: "⚡",
      title: "Lightning Fast",
      description: "Sub-100ms response times"
    },
    {
      icon: "🧠",
      title: "AI-Powered",
      description: "Neural search that learns"
    },
    {
      icon: "📚",
      title: "All Formats",
      description: "PDFs, DOCX, XLSX, images, text"
    }
  ];

  const comparisons = [
    { feature: "Data Location", shodh: "Your Device", chatgpt: "Cloud Servers" },
    { feature: "Internet Required", shodh: "No", chatgpt: "Always" },
    { feature: "Response Time", shodh: "<100ms", chatgpt: "2-5 seconds" },
    { feature: "Document Access", shodh: "Unlimited", chatgpt: "10MB limit" },
    { feature: "Privacy", shodh: "100% Private", chatgpt: "Data uploaded" },
    { feature: "Customization", shodh: "Full Control", chatgpt: "Limited" }
  ];

  const useCases = [
    { icon: "🏛️", title: "Government", desc: "Secure document management" },
    { icon: "⚖️", title: "Legal", desc: "Case law search" },
    { icon: "🏥", title: "Healthcare", desc: "Patient data analysis" },
    { icon: "🎓", title: "Education", desc: "Research organization" },
    { icon: "💼", title: "Enterprise", desc: "Knowledge management" },
    { icon: "🚀", title: "Startups", desc: "Cost-effective AI" }
  ];

  return (
    <div className="landing-container">
      {/* Animated background - Pure CSS */}
      <div className="animated-bg">
        <div className="gradient-orb orb-1"></div>
        <div className="gradient-orb orb-2"></div>
        <div className="gradient-orb orb-3"></div>
        <div className="grid-pattern"></div>
        
        {/* Animated particles */}
        <div className="particles">
          {[...Array(20)].map((_, i) => (
            <div key={i} className={`particle particle-${i + 1}`}></div>
          ))}
        </div>
        
        {/* Light streaks */}
        <div className="light-streaks">
          <div className="streak streak-1"></div>
          <div className="streak streak-2"></div>
          <div className="streak streak-3"></div>
          <div className="streak streak-4"></div>
          <div className="streak streak-5"></div>
          <div className="streak streak-6"></div>
          <div className="streak streak-7"></div>
          <div className="streak streak-8"></div>
        </div>
        
        {/* Glow lines */}
        <div className="glow-lines">
          <div className="glow-line horizontal"></div>
          <div className="glow-line vertical"></div>
        </div>
      </div>
      
      {/* Main Content - Single View */}
      <div className="landing-content">
        {/* Left Side - Hero */}
        <div className="hero-panel">
          <h1 className="brand-title">
            <div className="brand-with-logo">
              <img src="/shodh_logo_nobackground.svg" alt="Shodh" className="brand-logo" />
              <span className="brand-name">SHODH</span>
            </div>
            <span className="tagline">Document Intelligence for Enterprise</span>
          </h1>
          
          <p className="hero-description">
            Search, analyze, and chat with your documents using AI.
            100% local. Your data never leaves your device.
          </p>

          <div className="quick-stats">
            <div className="stat">
              <span className="stat-number">100%</span>
              <span className="stat-label">Private</span>
            </div>
            <div className="stat">
              <span className="stat-number">&lt;100ms</span>
              <span className="stat-label">Response</span>
            </div>
            <div className="stat">
              <span className="stat-number">10GB+</span>
              <span className="stat-label">Documents</span>
            </div>
          </div>

          <div className="cta-buttons">
            <button className="btn-start" onClick={onGetStarted}>
              Get Started
              <span className="btn-icon">→</span>
            </button>
          </div>

          <div className="trust-points">
            <span>✓ Works Offline</span>
            <span>✓ Enterprise Ready</span>
            <span>✓ Made in India</span>
          </div>
        </div>

        {/* Right Side - Interactive Content */}
        <div className="info-panel">
          {/* Tab Navigation */}
          <div className="tab-nav">
            <button 
              className={`tab ${activeTab === 'features' ? 'active' : ''}`}
              onClick={() => setActiveTab('features')}
            >
              Features
            </button>
            <button 
              className={`tab ${activeTab === 'compare' ? 'active' : ''}`}
              onClick={() => setActiveTab('compare')}
            >
              Compare
            </button>
            <button 
              className={`tab ${activeTab === 'demo' ? 'active' : ''}`}
              onClick={() => setActiveTab('demo')}
            >
              Demo
            </button>
            <button 
              className={`tab ${activeTab === 'industries' ? 'active' : ''}`}
              onClick={() => setActiveTab('industries')}
            >
              Industries
            </button>
          </div>

          {/* Tab Content */}
          <div className="tab-content">
            {activeTab === 'features' && (
              <div className="features-content">
                <h3>Why Shodh-RAG?</h3>
                <div className="features-grid">
                  {features.map((feature, index) => (
                    <div key={index} className="feature-item">
                      <span className="feature-icon">{feature.icon}</span>
                      <div className="feature-text">
                        <h4>{feature.title}</h4>
                        <p>{feature.description}</p>
                      </div>
                    </div>
                  ))}
                </div>
                <div className="feature-highlight">
                  <p>
                    <strong>Powered by advanced neural networks:</strong> Vamana indexing, 
                    ONNX embeddings, and hybrid search for unmatched performance.
                  </p>
                </div>
              </div>
            )}

            {activeTab === 'compare' && (
              <div className="compare-content">
                <h3>Shodh-RAG vs ChatGPT</h3>
                <div className="compare-table">
                  {comparisons.map((item, index) => (
                    <div key={index} className="compare-row">
                      <div className="compare-feature">{item.feature}</div>
                      <div className="compare-kalki">
                        <span className="check">✓</span> {item.shodh}
                      </div>
                      <div className="compare-other">{item.chatgpt}</div>
                    </div>
                  ))}
                </div>
                <p className="compare-note">
                  No monthly subscriptions. One-time setup for unlimited local AI.
                </p>
              </div>
            )}

            {activeTab === 'demo' && (
              <div className="demo-content">
                <h3>See It In Action</h3>
                <div className="demo-chat">
                  <div className="chat-msg user">
                    <span>👤</span>
                    <p>What are the key insights from our sales data?</p>
                  </div>
                  <div className="chat-msg bot">
                    <span>🤖</span>
                    <div>
                      <p><strong>Shodh-RAG:</strong> Based on your sales data:</p>
                      <ul>
                        <li>Q3 revenue: ₹12.5 Cr (23% YoY growth)</li>
                        <li>Top region: Maharashtra (45% of sales)</li>
                        <li>Digital channels: 67% contribution</li>
                      </ul>
                      <div className="sources">
                        <span>📄 sales-q3.xlsx</span>
                        <span>📊 regional-report.pdf</span>
                      </div>
                    </div>
                  </div>
                  <div className="demo-stats">
                    <span>⚡ 87ms response</span>
                    <span>📚 1,247 documents searched</span>
                  </div>
                </div>
              </div>
            )}

            {activeTab === 'industries' && (
              <div className="industries-content">
                <h3>Built for Indian Enterprises</h3>
                <div className="industries-grid">
                  {useCases.map((useCase, index) => (
                    <div key={index} className="industry-item">
                      <span className="industry-icon">{useCase.icon}</span>
                      <div className="industry-text">
                        <h4>{useCase.title}</h4>
                        <p>{useCase.desc}</p>
                      </div>
                    </div>
                  ))}
                </div>
                <p className="industry-note">
                  Compliant with Indian data protection regulations. 
                  Your sensitive data never leaves your premises.
                </p>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default LandingPage;